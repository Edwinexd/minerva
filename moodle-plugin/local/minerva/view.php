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
 * Embedded view of the Minerva AI assistant chat.
 *
 * Fetches an embed token via the integration API and loads the
 * Minerva embed frontend inside an iframe. The embed route is
 * separate from the SSO-protected routes and uses HMAC tokens.
 *
 * @package    local_minerva
 * @copyright  2026 DSV, Stockholm University
 * @license    http://www.gnu.org/copyleft/gpl.html GNU GPL v3 or later
 */

require_once(__DIR__ . '/../../config.php');

$courseid = required_param('id', PARAM_INT);

$course = get_course($courseid);
require_login($course);

$context = context_course::instance($courseid);
require_capability('local/minerva:view', $context);

$link = $DB->get_record('local_minerva_links', ['courseid' => $courseid]);
if (!$link) {
    throw new moodle_exception('no_link', 'local_minerva');
}

// Build the user's eppn the same way enrolment sync does.
global $USER;
$eppn = \local_minerva\observer::get_eppn($USER);
$displayname = fullname($USER);

// Fetch an embed token from the Minerva integration API.
$client = \local_minerva\api_client::from_link($link);
try {
    $tokendata = $client->create_embed_token($link->minerva_course_id, $eppn, $displayname);
} catch (\Exception $e) {
    throw new moodle_exception('api_error', 'local_minerva', '', null, $e->getMessage());
}

$apiurl = rtrim($link->minerva_api_url, '/');
// Build the frontend URL (strip /api suffix if present to get the base).
$frontendurl = preg_replace('#/api$#', '', $apiurl);
$chaturl = $frontendurl . '/embed/' . $link->minerva_course_id . '?token=' . urlencode($tokendata->token);

$PAGE->set_url(new moodle_url('/local/minerva/view.php', ['id' => $courseid]));
$PAGE->set_context($context);
$PAGE->set_course($course);
$PAGE->set_title(get_string('chat_title', 'local_minerva'));
$PAGE->set_heading($course->fullname . ' - ' . get_string('chat_title', 'local_minerva'));
$PAGE->set_pagelayout('incourse');

echo $OUTPUT->header();

echo html_writer::tag('p', get_string('chat_description', 'local_minerva'));

echo html_writer::tag('p',
    html_writer::link($chaturl, get_string('open_in_new_tab', 'local_minerva'), [
        'target' => '_blank',
        'class' => 'btn btn-secondary',
    ])
);

echo html_writer::tag('iframe', '', [
    'src' => $chaturl,
    'style' => 'width: 100%; height: 700px; border: 1px solid #dee2e6; border-radius: 8px;',
    'allow' => 'clipboard-write',
    'title' => get_string('chat_title', 'local_minerva'),
]);

echo $OUTPUT->footer();
