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
 * @copyright  2026 Edwin Sundberg
 * @license    http://www.gnu.org/copyleft/gpl.html GNU GPL v3 or later
 */

/**
 * Extend the course navigation with a Minerva link.
 *
 * @param navigation_node $parentnode
 * @param stdClass $course
 * @param context_course $context
 */
function local_minerva_extend_navigation_course(
    navigation_node $parentnode,
    stdClass $course,
    context_course $context
): void {
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

/**
 * Parse a Moodle course external id (`idnumber`) into Daisy offering ids.
 *
 * At DSV the course external id holds one or more Daisy `momenttillf_id`s
 * used to match the course to Daisy; it may be a single id or several
 * comma-separated (no spaces by convention, but we trim defensively).
 * Blank entries are dropped and duplicates collapsed, first-seen order kept.
 *
 * @param string|null $idnumber The course `idnumber` field.
 * @return string[] Normalised offering ids (possibly empty).
 */
function local_minerva_parse_external_ids(?string $idnumber): array {
    if ($idnumber === null || trim($idnumber) === '') {
        return [];
    }
    $ids = [];
    foreach (explode(',', $idnumber) as $part) {
        $id = trim($part);
        if ($id !== '' && !in_array($id, $ids, true)) {
            $ids[] = $id;
        }
    }
    return $ids;
}
