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
 * Library functions for local_minerva.
 *
 * @package    local_minerva
 * @copyright  2026 DSV, Stockholm University
 * @license    http://www.gnu.org/copyleft/gpl.html GNU GPL v3 or later
 */

defined('MOODLE_INTERNAL') || die();

/**
 * Extend the course navigation with a Minerva link.
 *
 * @param navigation_node $parentnode
 * @param stdClass $course
 * @param context_course $context
 */
function local_minerva_extend_navigation_course(navigation_node $parentnode, stdClass $course,
        context_course $context): void {
    global $DB;

    // Only show if the course is linked to Minerva.
    $link = $DB->get_record('local_minerva_links', ['courseid' => $course->id]);

    if ($link && has_capability('local/minerva:view', $context)) {
        $url = new moodle_url('/local/minerva/view.php', ['id' => $course->id]);
        $parentnode->add(
            get_string('minerva_assistant', 'local_minerva'),
            $url,
            navigation_node::TYPE_CUSTOM,
            null,
            'minerva_assistant',
            new pix_icon('i/star', '')
        );
    }

    if (has_capability('local/minerva:manage', $context)) {
        $url = new moodle_url('/local/minerva/manage.php', ['id' => $course->id]);
        $parentnode->add(
            get_string('manage_link', 'local_minerva'),
            $url,
            navigation_node::TYPE_SETTING,
            null,
            'minerva_manage',
            new pix_icon('i/settings', '')
        );
    }
}
