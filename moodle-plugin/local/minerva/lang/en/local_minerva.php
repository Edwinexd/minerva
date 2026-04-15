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
 * Language strings for local_minerva.
 *
 * @package    local_minerva
 * @copyright  2026 DSV, Stockholm University
 * @license    http://www.gnu.org/copyleft/gpl.html GNU GPL v3 or later
 */

defined('MOODLE_INTERNAL') || die();

$string['pluginname'] = 'Minerva AI Assistant';
$string['settings_apiurl'] = 'Minerva URL';
$string['settings_apiurl_desc'] = 'URL of the Minerva instance (e.g. https://minerva.dsv.su.se). Configured per course link.';
$string['settings_apiurl_admin_desc'] = 'If set, this URL is used for all course links and teachers cannot change it. Leave empty to let teachers enter the URL per course.';
$string['settings_apiurl_help'] = 'The URL of the Minerva instance (e.g. https://minerva.dsv.su.se). This is set per course by the teacher.';
$string['settings_public_url'] = 'Minerva public URL';
$string['settings_public_url_desc'] = 'Browser-accessible URL for the Minerva embed. Only needed when it differs from the Minerva URL above (e.g. local development). Leave empty in production.';
$string['settings_apikey'] = 'API key';
$string['settings_connection'] = 'Minerva connection';
$string['settings_autosync'] = 'Auto-sync enrolment';
$string['settings_autosync_desc'] = 'Automatically enrol/unenrol students in the linked Minerva course when they are enrolled/unenrolled in Moodle.';
$string['settings_autosync_materials'] = 'Auto-sync materials';
$string['settings_autosync_materials_desc'] = 'Automatically upload new resources from linked courses to Minerva every 30 minutes.';
$string['settings_eppn_suffix'] = 'EPPN suffix';
$string['settings_eppn_suffix_desc'] = 'Suffix appended to Moodle usernames to form the Shibboleth eppn (e.g. @SU.SE).';

// Capabilities.
$string['minerva:manage'] = 'Manage Minerva course link';
$string['minerva:view'] = 'View Minerva AI assistant';
$string['minerva:syncmaterials'] = 'Sync materials to Minerva';

// Navigation & UI.
$string['minerva_assistant'] = 'AI Assistant';
$string['manage_link'] = 'Minerva settings';
$string['link_course'] = 'Link Minerva course';
$string['unlink_course'] = 'Unlink';
$string['linked_course'] = 'Linked to Minerva course';
$string['no_link'] = 'This course is not linked to a Minerva course.';
$string['select_minerva_course'] = 'Select Minerva course';
$string['link_saved'] = 'Course link saved.';
$string['link_removed'] = 'Course link removed.';
$string['sync_enrolment'] = 'Sync enrolment now';
$string['sync_enrolment_done'] = 'Enrolment sync complete: {$a->added} added, {$a->removed} removed.';
$string['sync_materials'] = 'Sync materials';
$string['sync_materials_desc'] = 'Upload course resources (files, URLs, pages, books, labels, section summaries) to the linked Minerva course.';
$string['sync_materials_done'] = 'Material sync complete: {$a->uploaded} resource(s) uploaded.';
$string['sync_materials_none'] = 'No new resources to sync.';
$string['reset_sync_log'] = 'Reset sync log';
$string['reset_sync_log_confirm'] = 'This clears the record of what has been synced so the next sync re-uploads everything. Continue?';
$string['sync_log_reset_done'] = 'Cleared {$a->count} sync-log entries. Next sync will re-upload everything.';
$string['unlink_course_confirm'] = 'This removes the link and clears the sync log. Members already in Minerva stay there. Continue?';
$string['no_api_configured'] = 'Minerva API credentials are missing or invalid. Please check the API URL and key in the course link settings.';
$string['invalid_api_url'] = 'Invalid Minerva API URL.';
$string['insecure_api_url'] = 'Minerva API URL must use https (http is only allowed for localhost).';
$string['no_scoped_course'] = 'The API key is not scoped to any Minerva course. Please check the key in the Minerva teacher dashboard.';
$string['minerva_course_id'] = 'Minerva course ID';
$string['minerva_course_id_help'] = 'The UUID of the Minerva course to link to (e.g. a1b2c3d4-e5f6-7890-abcd-ef1234567890). You can find this in the Minerva teacher dashboard.';
$string['invalid_uuid'] = 'Invalid course ID format. Please enter a valid UUID.';
$string['connection_failed'] = 'Could not connect to Minerva: {$a}';
$string['chat_title'] = 'Minerva AI Assistant';
$string['chat_description'] = 'Ask questions about the course material.';
$string['open_in_new_tab'] = 'Open in new tab';
$string['privacy:metadata'] = 'The Minerva plugin sends user identifiers (eppn) to the external Minerva service for authentication and enrolment sync.';

// Tasks.
$string['task_sync_enrolments'] = 'Sync enrolments to Minerva';
$string['task_sync_materials'] = 'Sync materials to Minerva';
