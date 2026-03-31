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
 * Library functions for mod_minerva.
 *
 * DEPRECATED: This activity module is deprecated. Use LTI 1.3 integration
 * instead, which provides embedded chat directly from the LMS without needing
 * a separate activity module. See the LTI tab in Minerva's course settings.
 *
 * @package    mod_minerva
 * @deprecated Since 0.2.0. Use LTI 1.3 integration instead.
 * @copyright  2026 DSV, Stockholm University
 * @license    http://www.gnu.org/copyleft/gpl.html GNU GPL v3 or later
 */

/**
 * Add a new Minerva activity instance.
 *
 * @param stdClass $data Form data.
 * @param mod_minerva_mod_form $mform The form.
 * @return int New instance ID.
 */
function minerva_add_instance(stdClass $data, $mform = null): int {
    global $DB;

    $data->timecreated = time();
    $data->timemodified = time();

    return $DB->insert_record('minerva', $data);
}

/**
 * Update an existing Minerva activity instance.
 *
 * @param stdClass $data Form data.
 * @param mod_minerva_mod_form $mform The form.
 * @return bool True on success.
 */
function minerva_update_instance(stdClass $data, $mform = null): bool {
    global $DB;

    $data->timemodified = time();
    $data->id = $data->instance;

    return $DB->update_record('minerva', $data);
}

/**
 * Delete a Minerva activity instance.
 *
 * @param int $id Instance ID.
 * @return bool True on success.
 */
function minerva_delete_instance(int $id): bool {
    global $DB;

    return $DB->delete_records('minerva', ['id' => $id]);
}

/**
 * Supported features.
 *
 * @param string $feature FEATURE_xx constant.
 * @return mixed True if supported, null otherwise.
 */
function minerva_supports(string $feature) {
    $features = [
        FEATURE_MOD_INTRO => true,
        FEATURE_SHOW_DESCRIPTION => true,
        FEATURE_BACKUP_MOODLE2 => true,
    ];
    return $features[$feature] ?? null;
}
