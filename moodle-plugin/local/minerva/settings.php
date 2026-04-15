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
 * Admin settings for the Minerva integration plugin.
 *
 * @package    local_minerva
 * @copyright  2026 DSV, Stockholm University
 * @license    http://www.gnu.org/copyleft/gpl.html GNU GPL v3 or later
 */

defined('MOODLE_INTERNAL') || die();

// Settings.php is loaded before the class autoloader is fully primed on
// fresh admin pages, so pull the helper class in explicitly.
require_once(__DIR__ . '/classes/admin_setting_httpsurl.php');

if ($hassiteconfig) {
    $settings = new admin_settingpage('local_minerva', get_string('pluginname', 'local_minerva'));

    // URL setting that also rejects non-https schemes (except for localhost),
    // since the API key is sent in the Authorization header.
    $settings->add(new \local_minerva\admin_setting_httpsurl(
        'local_minerva/minerva_url',
        get_string('settings_apiurl', 'local_minerva'),
        get_string('settings_apiurl_admin_desc', 'local_minerva'),
        '',
        PARAM_URL
    ));

    // Auto-sync enrollment on enrol/unenrol events.
    $settings->add(new admin_setting_configcheckbox(
        'local_minerva/autosync_enrolment',
        get_string('settings_autosync', 'local_minerva'),
        get_string('settings_autosync_desc', 'local_minerva'),
        1
    ));

    // Auto-sync materials on schedule.
    $settings->add(new admin_setting_configcheckbox(
        'local_minerva/autosync_materials',
        get_string('settings_autosync_materials', 'local_minerva'),
        get_string('settings_autosync_materials_desc', 'local_minerva'),
        1
    ));

    // EPPN suffix (e.g. @SU.SE) appended to Moodle usernames.
    $settings->add(new admin_setting_configtext(
        'local_minerva/eppn_suffix',
        get_string('settings_eppn_suffix', 'local_minerva'),
        get_string('settings_eppn_suffix_desc', 'local_minerva'),
        '@SU.SE',
        PARAM_TEXT
    ));

    $ADMIN->add('localplugins', $settings);
}
