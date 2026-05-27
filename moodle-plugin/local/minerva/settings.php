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
 * @copyright  2026 Edwin Sundberg
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

    // Optional site-level integration key: when set, teachers no longer need
    // to visit Minerva to create an API key before linking a course; the
    // plugin calls /api/integration/site/provision on their behalf and
    // stores the resulting per-course key. Admins mint this in the Minerva
    // "Plugin Integrations" admin page. Leave blank to keep the legacy
    // per-course paste flow.
    $settings->add(new admin_setting_configpasswordunmask(
        'local_minerva/site_api_key',
        get_string('settings_site_api_key', 'local_minerva'),
        get_string('settings_site_api_key_desc', 'local_minerva'),
        ''
    ));

    // Auto-sync materials on schedule.
    $settings->add(new admin_setting_configcheckbox(
        'local_minerva/autosync_materials',
        get_string('settings_autosync_materials', 'local_minerva'),
        get_string('settings_autosync_materials_desc', 'local_minerva'),
        1
    ));

    // Slice 3: site-level kill switch for forum sync. Default ON so
    // installations get the feature out of the box, but admins can flip
    // it off across the whole site for compliance or policy reasons.
    // Per-course sync is OFF by default and only togglable when this
    // is ON (enforced in manage.php's checkbox + the sync task's
    // gate); flipping this OFF immediately disables forum sync on all
    // courses regardless of their per-link setting.
    $settings->add(new admin_setting_configcheckbox(
        'local_minerva/enable_forum_sync',
        get_string('settings_enable_forum_sync', 'local_minerva'),
        get_string('settings_enable_forum_sync_desc', 'local_minerva'),
        1
    ));

    $ADMIN->add('localplugins', $settings);
}
