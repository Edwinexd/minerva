<?php
// This file is part of Moodle - http://moodle.org/
//
// Moodle is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// Moodle is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with Moodle.  If not, see <http://www.gnu.org/licenses/>.

/**
 * Manage the link between a Moodle course and a Minerva course.
 *
 * Teachers configure the Minerva API URL, API key, and select a course.
 * Credentials are stored per course link, not globally.
 *
 * @package    local_minerva
 * @copyright  2026 Edwin Sundberg
 * @license    http://www.gnu.org/copyleft/gpl.html GNU GPL v3 or later
 */

require_once(__DIR__ . '/../../config.php');
// The lib.php file holds local_minerva_parse_external_ids(); Moodle only
// loads it lazily (via nav callbacks), too late for the action handlers
// below, so require it explicitly.
require_once(__DIR__ . '/lib.php');

$courseid = required_param('id', PARAM_INT);
$action = optional_param('action', '', PARAM_ALPHA);

$course = get_course($courseid);
require_login($course);

$context = context_course::instance($courseid);
require_capability('local/minerva:manage', $context);

$pageurl = new moodle_url('/local/minerva/manage.php', ['id' => $courseid]);
$PAGE->set_url($pageurl);
$PAGE->set_context($context);
$PAGE->set_course($course);
$PAGE->set_title(get_string('manage_link', 'local_minerva'));
$PAGE->set_heading($course->fullname . ' - ' . get_string('manage_link', 'local_minerva'));
$PAGE->set_pagelayout('incourse');

// Mutating actions must be POST + sesskey.
if ($action !== '') {
    require_sesskey();
    if (!in_array($_SERVER['REQUEST_METHOD'] ?? '', ['POST'], true)) {
        throw new moodle_exception('invalidrequest');
    }
}

if ($action === 'unlink') {
    $DB->delete_records('local_minerva_links', ['courseid' => $courseid]);
    // Drop the per-course sync log too, so re-linking doesn't silently
    // assume content already lives in Minerva.
    $DB->delete_records('local_minerva_sync_log', ['courseid' => $courseid]);
    // Treat an explicit unlink as opting out of external-id auto-linking, so
    // the sweep doesn't re-link a still-matched course behind the teacher's
    // back. Re-linking (manually or via auto-connect) clears this.
    if (!$DB->record_exists('local_minerva_autolink_optout', ['courseid' => $courseid])) {
        $DB->insert_record(
            'local_minerva_autolink_optout',
            (object) ['courseid' => $courseid, 'timecreated' => time()]
        );
    }
    redirect(
        $pageurl,
        get_string('link_removed', 'local_minerva'),
        null,
        \core\output\notification::NOTIFY_SUCCESS
    );
}

if ($action === 'resetsync') {
    $count = $DB->count_records('local_minerva_sync_log', ['courseid' => $courseid]);
    $DB->delete_records('local_minerva_sync_log', ['courseid' => $courseid]);
    redirect(
        $pageurl,
        get_string('sync_log_reset_done', 'local_minerva', (object)['count' => $count]),
        null,
        \core\output\notification::NOTIFY_SUCCESS
    );
}

// Slice 3: per-course forum sync toggle. Gated on the site-level
// `enable_forum_sync` admin setting; if the admin has it OFF this
// action 4xxs even if the teacher knows the URL ; the UI hides the
// control too, so this is defence in depth.
if ($action === 'syncforums') {
    if (!get_config('local_minerva', 'enable_forum_sync')) {
        throw new \moodle_exception('forum_sync_disabled_site', 'local_minerva');
    }
    $value = optional_param('value', 0, PARAM_BOOL) ? 1 : 0;
    $linkrow = $DB->get_record('local_minerva_links', ['courseid' => $courseid], '*', MUST_EXIST);
    $linkrow->sync_forums = $value;
    $linkrow->timemodified = time();
    $DB->update_record('local_minerva_links', $linkrow);
    $msg = $value
        ? get_string('forum_sync_enabled_course', 'local_minerva')
        : get_string('forum_sync_disabled_course', 'local_minerva');
    redirect($pageurl, $msg, null, \core\output\notification::NOTIFY_SUCCESS);
}

// Auto-connect: resolve the Moodle course external id (Daisy offering ids)
// to a Minerva course and provision a per-course key in one step. Only
// available in site-integration mode; mirrors what the picker + provision
// flow does, keyed off the external id instead of a manual selection.
if ($action === 'autoconnect') {
    if (!\local_minerva\api_client::site_integration_available()) {
        throw new \moodle_exception('site_integration_not_configured', 'local_minerva');
    }
    // Never clobber an existing link.
    if ($DB->record_exists('local_minerva_links', ['courseid' => $courseid])) {
        redirect($pageurl);
    }
    $ids = local_minerva_parse_external_ids($course->idnumber);
    if (empty($ids)) {
        redirect(
            $pageurl,
            get_string('autoconnect_no_external_id', 'local_minerva'),
            null,
            \core\output\notification::NOTIFY_ERROR
        );
    }
    $idlist = implode(', ', $ids);
    try {
        $client = \local_minerva\api_client::from_site_config();
        $res = $client->site_resolve_offerings($USER->username, $ids);
    } catch (\Exception $e) {
        redirect(
            $pageurl,
            get_string('connection_failed', 'local_minerva', s($e->getMessage())),
            null,
            \core\output\notification::NOTIFY_ERROR
        );
    }
    $status = $res->status ?? 'none';
    if ($status !== 'matched' || empty($res->course)) {
        // A missing Minerva account looks like "no match" but needs the
        // opposite advice (log in first), so split the two cases.
        $msg = empty($res->user_exists)
            ? get_string('site_user_not_found', 'local_minerva')
            : get_string('autoconnect_none', 'local_minerva', s($idlist));
        redirect($pageurl, $msg, null, \core\output\notification::NOTIFY_ERROR);
    }
    if (empty($res->authorized)) {
        redirect(
            $pageurl,
            get_string('autoconnect_not_authorized', 'local_minerva'),
            null,
            \core\output\notification::NOTIFY_ERROR
        );
    }
    $mc = $res->course;
    // Provision the per-course key. The server re-checks authorization, so an
    // ACL race surfaces here rather than leaving a half-baked link row.
    try {
        $minted = $client->site_provision_course_key(
            $USER->username,
            format_string($course->fullname),
            $mc->id
        );
    } catch (\Exception $e) {
        redirect(
            $pageurl,
            get_string('connection_failed', 'local_minerva', s($e->getMessage())),
            null,
            \core\output\notification::NOTIFY_ERROR
        );
    }
    if (empty($minted->key)) {
        redirect(
            $pageurl,
            get_string('site_provision_empty_key', 'local_minerva'),
            null,
            \core\output\notification::NOTIFY_ERROR
        );
    }

    $record = new stdClass();
    $record->courseid = $courseid;
    $record->minerva_course_id = $mc->id;
    $record->minerva_course_name = $mc->name;
    $record->minerva_api_url = rtrim(get_config('local_minerva', 'minerva_url'), '/');
    $record->minerva_api_key = $minted->key;
    $record->timecreated = time();
    $record->timemodified = time();
    // A concurrent double-submit could have inserted the link between our
    // record_exists check and here (courseid is unique). Treat the collision
    // as success rather than surfacing a raw DB error page.
    try {
        $DB->insert_record('local_minerva_links', $record);
    } catch (\dml_write_exception $e) {
        if ($DB->record_exists('local_minerva_links', ['courseid' => $courseid])) {
            redirect($pageurl);
        }
        throw $e;
    }
    // Linking by hand is an explicit opt-in; clear any prior unlink opt-out.
    $DB->delete_records('local_minerva_autolink_optout', ['courseid' => $courseid]);

    redirect(
        $pageurl,
        get_string('autoconnect_done', 'local_minerva', s($mc->name)),
        null,
        \core\output\notification::NOTIFY_SUCCESS
    );
}

echo $OUTPUT->header();

// Data-handling disclosure shown on every view so teachers re-see it on
// each visit, not only at initial link time.
echo html_writer::tag(
    'div',
    html_writer::tag('strong', get_string('datahandling_heading', 'local_minerva')) .
        html_writer::empty_tag('br') .
        html_writer::tag(
            'ul',
            html_writer::tag('li', get_string('datahandling_materials', 'local_minerva')) .
                html_writer::tag('li', get_string('datahandling_inference', 'local_minerva')) .
                html_writer::tag('li', get_string('datahandling_apikey', 'local_minerva'))
        ),
    ['class' => 'alert alert-warning']
);

// Get current link.
$link = $DB->get_record('local_minerva_links', ['courseid' => $courseid]);

if ($link) {
    // Show current link and management options.
    echo html_writer::tag(
        'div',
        html_writer::tag('strong', get_string('linked_course', 'local_minerva') . ': ') .
            s($link->minerva_course_name) .
            ' (' . s($link->minerva_course_id) . ')' .
            html_writer::empty_tag('br') .
            html_writer::tag(
                'small',
                get_string('settings_apiurl', 'local_minerva') . ': ' . s($link->minerva_api_url),
                ['class' => 'text-muted']
            ),
        ['class' => 'alert alert-info']
    );

    // Note when the link was created by the external-id auto-linker, and
    // explain that unlinking is a durable opt-out.
    if (!empty($link->auto_linked)) {
        echo html_writer::div(
            get_string('linked_auto_note', 'local_minerva'),
            'alert alert-secondary'
        );
    }

    // Unlink (destructive: confirm + red).
    $unlinkbtn = new \core\output\single_button(
        new moodle_url($pageurl, ['action' => 'unlink']),
        get_string('unlink_course', 'local_minerva'),
        'post',
        \core\output\single_button::BUTTON_DANGER
    );
    $unlinkbtn->add_confirm_action(get_string('unlink_course_confirm', 'local_minerva'));
    echo $OUTPUT->render($unlinkbtn);

    // Sync materials button.
    if (has_capability('local/minerva:syncmaterials', $context)) {
        $maturl = new moodle_url('/local/minerva/sync.php', ['id' => $courseid]);
        echo html_writer::link($maturl, get_string('sync_materials', 'local_minerva'), [
            'class' => 'btn btn-secondary',
        ]);

        $resetbtn = new \core\output\single_button(
            new moodle_url($pageurl, ['action' => 'resetsync']),
            get_string('reset_sync_log', 'local_minerva'),
            'post',
            \core\output\single_button::BUTTON_DANGER
        );
        $resetbtn->add_confirm_action(get_string('reset_sync_log_confirm', 'local_minerva'));
        echo $OUTPUT->render($resetbtn);

        // Slice 3: forum sync toggle. Only visible when the site-level
        // setting allows it ; we don't even render the control when
        // it's globally off, so teachers don't see a perpetually-
        // disabled checkbox they can never use. With the site setting
        // on, teachers see a clear per-course toggle that explains the
        // privacy posture (teacher-answered threads only, PII-scrubbed).
        if (get_config('local_minerva', 'enable_forum_sync')) {
            $enabled = !empty($link->sync_forums);
            $togglelabel = $enabled
                ? get_string('forum_sync_disable_btn', 'local_minerva')
                : get_string('forum_sync_enable_btn', 'local_minerva');
            $confirmstr = $enabled
                ? get_string('forum_sync_disable_confirm', 'local_minerva')
                : get_string('forum_sync_enable_confirm', 'local_minerva');
            $togglebtn = new \core\output\single_button(
                new moodle_url($pageurl, [
                    'action' => 'syncforums',
                    'value' => $enabled ? 0 : 1,
                ]),
                $togglelabel,
                'post'
            );
            $togglebtn->add_confirm_action($confirmstr);
            echo html_writer::tag(
                'div',
                html_writer::tag('strong', get_string('forum_sync_section_title', 'local_minerva')) .
                    html_writer::empty_tag('br') .
                    html_writer::tag(
                        'span',
                        $enabled
                            ? get_string('forum_sync_status_on', 'local_minerva')
                            : get_string('forum_sync_status_off', 'local_minerva')
                    ) .
                    html_writer::empty_tag('br') .
                    html_writer::tag(
                        'small',
                        get_string('forum_sync_blurb', 'local_minerva'),
                        ['class' => 'text-muted']
                    ),
                ['class' => 'mt-3 mb-2']
            );
            echo $OUTPUT->render($togglebtn);
        }
    }
} else {
    // Auto-connect offer: in site-integration mode, if this Moodle course
    // carries an external id (Daisy offering ids), resolve it to a Minerva
    // course so the teacher can link in one click. Falls through to the
    // manual picker/form below in every non-matched case.
    $extids = local_minerva_parse_external_ids($course->idnumber);
    if (\local_minerva\api_client::site_integration_available() && !empty($extids)) {
        $idlist = implode(', ', $extids);
        $autolinkon = (bool) get_config('local_minerva', 'autolink_by_external_id');
        $optedout = $DB->record_exists('local_minerva_autolink_optout', ['courseid' => $courseid]);
        try {
            $client = \local_minerva\api_client::from_site_config();
            $res = $client->site_resolve_offerings($USER->username, $extids);
            $status = $res->status ?? 'none';
            if ($status === 'matched' && !empty($res->course)) {
                $mc = $res->course;
                $authorized = !empty($res->authorized);
                // Will the unattended sweep link this course on its own?
                $willauto = $autolinkon && !$optedout;

                echo html_writer::tag(
                    'div',
                    html_writer::tag('strong', get_string('autoconnect_section_title', 'local_minerva')) .
                        html_writer::empty_tag('br') .
                        get_string('autoconnect_matched', 'local_minerva', (object)[
                            'ids' => s($idlist),
                            'name' => s($mc->name),
                        ]),
                    ['class' => 'alert alert-success']
                );

                if (!empty($res->multiple_matches)) {
                    echo html_writer::div(
                        get_string('autoconnect_multiple_matches', 'local_minerva', s($mc->name)),
                        'alert alert-info'
                    );
                }

                if ($willauto) {
                    echo html_writer::div(
                        get_string('autoconnect_will_autolink', 'local_minerva'),
                        'alert alert-info'
                    );
                } else if ($autolinkon && $optedout) {
                    echo html_writer::div(
                        get_string('autoconnect_optedout', 'local_minerva'),
                        'alert alert-warning'
                    );
                }

                if ($authorized) {
                    $connectbtn = new \core\output\single_button(
                        new moodle_url($pageurl, ['action' => 'autoconnect']),
                        // The single_button helper escapes its label; pass raw name.
                        get_string('autoconnect_btn', 'local_minerva', $mc->name),
                        'post'
                    );
                    $connectbtn->add_confirm_action(get_string('autoconnect_confirm', 'local_minerva'));
                    echo $OUTPUT->render($connectbtn);
                } else if (!$willauto) {
                    // Can't self-provision and nothing automatic will happen.
                    echo html_writer::div(
                        get_string('autoconnect_not_authorized', 'local_minerva'),
                        'alert alert-warning'
                    );
                }
            } else if (empty($res->user_exists)) {
                // Maps to nothing only because the teacher has no Minerva
                // account yet; point them at logging in rather than the import.
                echo html_writer::div(
                    get_string('site_user_not_found', 'local_minerva'),
                    'alert alert-warning'
                );
            } else {
                echo html_writer::div(
                    get_string('autoconnect_none', 'local_minerva', s($idlist)),
                    'alert alert-info'
                );
            }
        } catch (\Exception $e) {
            // Non-fatal: surface it but let the manual form still render.
            echo html_writer::div(
                get_string('connection_failed', 'local_minerva', s($e->getMessage())),
                'alert alert-warning'
            );
        }
    }

    // Link form: just URL (if not locked) + API key.
    // The key is scoped to a single course, so we resolve it automatically.
    $form = new \local_minerva\form\link_course_form($pageurl);

    if ($form->is_cancelled()) {
        redirect(new moodle_url('/course/view.php', ['id' => $courseid]));
    }

    if ($data = $form->get_data()) {
        // Validation has already resolved the scoped course; reuse it instead
        // of calling the API a second time.
        $mc = $form->resolvedcourse;
        if ($mc === null) {
            // Defensive: should not happen (validation ran), but guard anyway.
            redirect(
                $pageurl,
                get_string('no_scoped_course', 'local_minerva'),
                null,
                \core\output\notification::NOTIFY_ERROR
            );
        }

        // Site-integration mode: the form already provisioned a per-course
        // key; fall back to the teacher-pasted key when running in legacy mode.
        $apikey = $form->provisionedkey ?? ($data->minerva_api_key ?? null);
        if (empty($apikey)) {
            redirect(
                $pageurl,
                get_string('no_api_configured', 'local_minerva'),
                null,
                \core\output\notification::NOTIFY_ERROR
            );
        }

        $record = new stdClass();
        $record->courseid = $courseid;
        $record->minerva_course_id = $mc->id;
        $record->minerva_course_name = $mc->name;
        $record->minerva_api_url = rtrim($data->minerva_api_url, '/');
        $record->minerva_api_key = $apikey;
        $record->timecreated = time();
        $record->timemodified = time();
        $DB->insert_record('local_minerva_links', $record);
        // Manual link is an explicit opt-in; clear any prior unlink opt-out.
        $DB->delete_records('local_minerva_autolink_optout', ['courseid' => $courseid]);

        redirect(
            $pageurl,
            get_string('link_saved', 'local_minerva'),
            null,
            \core\output\notification::NOTIFY_SUCCESS
        );
    }

    $form->set_data(['courseid' => $courseid]);
    $form->display();
}

echo $OUTPUT->footer();
