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
 * Plugin version and other metadata.
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

defined('MOODLE_INTERNAL') || die();

$plugin->component = 'mod_minerva';
$plugin->version   = 2026033100;
$plugin->requires  = 2022112800; // Moodle 4.1+.
$plugin->maturity  = MATURITY_ALPHA;
$plugin->release   = '0.2.0-deprecated';
$plugin->dependencies = [
    'local_minerva' => 2026033000,
];
