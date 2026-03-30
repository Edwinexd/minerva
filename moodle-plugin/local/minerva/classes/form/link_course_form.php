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

namespace local_minerva\form;

defined('MOODLE_INTERNAL') || die();

require_once($CFG->libdir . '/formslib.php');

/**
 * Form to link a Moodle course to a Minerva course.
 *
 * @package    local_minerva
 * @copyright  2026 DSV, Stockholm University
 * @license    http://www.gnu.org/copyleft/gpl.html GNU GPL v3 or later
 */
class link_course_form extends \moodleform {

    /**
     * Define the form elements.
     */
    protected function definition(): void {
        $mform = $this->_form;
        $courses = $this->_customdata['minerva_courses'] ?? [];

        $options = ['' => get_string('select_minerva_course', 'local_minerva')];
        foreach ($courses as $course) {
            $label = $course->name;
            if (!empty($course->description)) {
                $label .= ' - ' . shorten_text($course->description, 60);
            }
            $options[$course->id] = $label;
        }

        $mform->addElement('select', 'minerva_course_id',
            get_string('select_minerva_course', 'local_minerva'), $options);
        $mform->addRule('minerva_course_id', null, 'required', null, 'client');

        $mform->addElement('hidden', 'courseid');
        $mform->setType('courseid', PARAM_INT);

        $this->add_action_buttons(true, get_string('link_course', 'local_minerva'));
    }
}
