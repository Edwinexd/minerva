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
 * View the Minerva AI assistant chat embedded in the course.
 *
 * Fetches an embed token via the integration API and loads the
 * Minerva chat frontend inside an iframe.
 *
 * @package    mod_minerva
 * @copyright  2026 DSV, Stockholm University
 * @license    http://www.gnu.org/copyleft/gpl.html GNU GPL v3 or later
 */

require_once(__DIR__ . '/../../config.php');

$id = required_param('id', PARAM_INT); // Course module ID.

$cm = get_coursemodule_from_id('minerva', $id, 0, false, MUST_EXIST);
$course = get_course($cm->course);
$instance = $DB->get_record('minerva', ['id' => $cm->instance], '*', MUST_EXIST);

require_login($course, true, $cm);

$context = context_module::instance($cm->id);
require_capability('mod/minerva:view', $context);

// Look up the course link from local_minerva.
$link = $DB->get_record('local_minerva_links', ['courseid' => $course->id]);
if (!$link) {
    throw new moodle_exception('no_course_link', 'mod_minerva');
}

// Build the user's eppn.
global $USER;
$eppn = \local_minerva\observer::get_eppn($USER);
$displayname = fullname($USER);

// Fetch an embed token from the Minerva integration API.
$client = \local_minerva\api_client::from_link($link);
$tokendata = $client->create_embed_token($link->minerva_course_id, $eppn, $displayname);

// Use public URL if set (for local dev where browser URL differs from internal API URL).
// Falls back to the API URL with /api stripped.
$publicurl = get_config('local_minerva', 'minerva_public_url');
if (empty($publicurl)) {
    $publicurl = preg_replace('#/api$#', '', rtrim($link->minerva_api_url, '/'));
}
$chaturl = rtrim($publicurl, '/') . '/embed/' . $link->minerva_course_id . '?token=' . urlencode($tokendata->token);

$PAGE->set_url(new moodle_url('/mod/minerva/view.php', ['id' => $id]));
$PAGE->set_context($context);
$PAGE->set_title($instance->name);
$PAGE->set_heading($course->fullname);
$PAGE->set_activity_record($instance);

echo $OUTPUT->header();

echo html_writer::tag(
    'p',
    html_writer::link($chaturl, get_string('open_in_new_tab', 'mod_minerva'), [
        'target' => '_blank',
        'class' => 'btn btn-secondary btn-sm',
    ])
);

echo html_writer::tag('iframe', '', [
    'src' => $chaturl,
    'style' => 'width: 100%; height: 700px; border: 1px solid #dee2e6; border-radius: 8px;',
    'allow' => 'clipboard-write',
    'title' => $instance->name,
]);

echo $OUTPUT->footer();
