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
 * Teachers provide their Minerva API URL, API key, and course UUID.
 * Each course link stores its own credentials.
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

        // Connection settings.
        $mform->addElement('header', 'connectionhdr',
            get_string('settings_connection', 'local_minerva'));

        $mform->addElement('text', 'minerva_api_url',
            get_string('settings_apiurl', 'local_minerva'), ['size' => 60]);
        $mform->setType('minerva_api_url', PARAM_URL);
        $mform->addRule('minerva_api_url', null, 'required', null, 'client');
        $mform->addHelpButton('minerva_api_url', 'settings_apiurl', 'local_minerva');

        $mform->addElement('passwordunmask', 'minerva_api_key',
            get_string('settings_apikey', 'local_minerva'), ['size' => 60]);
        $mform->setType('minerva_api_key', PARAM_RAW);
        $mform->addRule('minerva_api_key', null, 'required', null, 'client');

        // Course selection.
        $mform->addElement('header', 'coursehdr',
            get_string('select_minerva_course', 'local_minerva'));

        // If we already have a list of courses (second step), show dropdown.
        $courses = $this->_customdata['minerva_courses'] ?? null;
        if ($courses !== null) {
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
        } else {
            // First step: text field for course UUID.
            $mform->addElement('text', 'minerva_course_id',
                get_string('minerva_course_id', 'local_minerva'), ['size' => 40]);
            $mform->addHelpButton('minerva_course_id', 'minerva_course_id', 'local_minerva');
        }
        $mform->setType('minerva_course_id', PARAM_RAW);
        $mform->addRule('minerva_course_id', null, 'required', null, 'client');

        $mform->addElement('hidden', 'courseid');
        $mform->setType('courseid', PARAM_INT);

        $this->add_action_buttons(true, get_string('link_course', 'local_minerva'));
    }

    /**
     * Validate the form data.
     *
     * @param array $data
     * @param array $files
     * @return array Errors keyed by field name.
     */
    public function validation($data, $files): array {
        $errors = parent::validation($data, $files);

        // Validate UUID format.
        if (!empty($data['minerva_course_id'])) {
            $uuid = trim($data['minerva_course_id']);
            if (!preg_match('/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i', $uuid)) {
                $errors['minerva_course_id'] = get_string('invalid_uuid', 'local_minerva');
            }
        }

        // Test the connection if URL and key are provided.
        if (!empty($data['minerva_api_url']) && !empty($data['minerva_api_key']) && empty($errors)) {
            try {
                $client = new \local_minerva\api_client($data['minerva_api_url'], $data['minerva_api_key']);
                $client->list_courses();
            } catch (\Exception $e) {
                $errors['minerva_api_key'] = get_string('connection_failed', 'local_minerva',
                    $e->getMessage());
            }
        }

        return $errors;
    }
}
