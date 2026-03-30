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
$string['settings_apiurl'] = 'Minerva API URL';
$string['settings_apiurl_desc'] = 'Base URL of the Minerva instance (e.g. https://minerva.dsv.su.se/api).';
$string['settings_apikey'] = 'API key';
$string['settings_apikey_desc'] = 'API key for the Minerva integration endpoints (MINERVA_API_KEY).';
$string['settings_autosync'] = 'Auto-sync enrolment';
$string['settings_autosync_desc'] = 'Automatically enrol/unenrol students in the linked Minerva course when they are enrolled/unenrolled in Moodle.';
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
$string['sync_materials_desc'] = 'Upload course files to the linked Minerva course for RAG processing.';
$string['sync_materials_done'] = 'Material sync complete: {$a->uploaded} files uploaded.';
$string['sync_materials_none'] = 'No new files to sync.';
$string['no_api_configured'] = 'Minerva integration is not configured. Please set the API URL and key in Site administration > Plugins > Local plugins > Minerva AI Assistant.';
$string['chat_title'] = 'Minerva AI Assistant';
$string['chat_description'] = 'Ask questions about the course material.';
$string['open_in_new_tab'] = 'Open in new tab';
$string['privacy:metadata'] = 'The Minerva plugin sends user identifiers (eppn) to the external Minerva service for authentication and enrolment sync.';

// Task.
$string['task_sync_enrolments'] = 'Sync enrolments to Minerva';
