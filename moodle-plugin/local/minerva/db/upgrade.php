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
 * Plugin upgrade steps for local_minerva.
 *
 * @package    local_minerva
 * @copyright  2026 DSV, Stockholm University
 * @license    http://www.gnu.org/copyleft/gpl.html GNU GPL v3 or later
 */

/**
 * Upgrade callback.
 *
 * @param int $oldversion
 * @return bool
 */
function xmldb_local_minerva_upgrade(int $oldversion): bool {
    global $DB;

    $dbman = $DB->get_manager();

    // Slice 2: add `sourceref` to the sync log so the plugin can build
    // the reconcile-sweep payload from previously-uploaded items even
    // when they weren't re-discovered this run. Pre-rollout rows are
    // left with NULL sourceref ; the reconcile sweep ignores them
    // (see sync_materials::current_source_refs) so they don't get
    // orphaned by accident.
    if ($oldversion < 2026052401) {
        $table = new xmldb_table('local_minerva_sync_log');
        $field = new xmldb_field('sourceref', XMLDB_TYPE_CHAR, 255, null, null, null, null, 'contenthash');
        if (!$dbman->field_exists($table, $field)) {
            $dbman->add_field($table, $field);
        }
        $index = new xmldb_index('courseid_sourceref', XMLDB_INDEX_NOTUNIQUE, ['courseid', 'sourceref']);
        if (!$dbman->index_exists($table, $index)) {
            $dbman->add_index($table, $index);
        }
        upgrade_plugin_savepoint(true, 2026052401, 'local', 'minerva');
    }

    return true;
}
