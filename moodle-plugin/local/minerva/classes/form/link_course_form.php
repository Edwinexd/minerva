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
 * Teacher enters the API key (and URL if not locked by site admin).
 * The key is scoped to a single Minerva course, so the course is
 * resolved automatically from the API.
 *
 * @package    local_minerva
 * @copyright  2026 DSV, Stockholm University
 * @license    http://www.gnu.org/copyleft/gpl.html GNU GPL v3 or later
 */
class link_course_form extends \moodleform {
    /**
     * Scoped course resolved from the API key during validation.
     * Cached so manage.php does not need to re-call list_courses().
     *
     * @var object|null
     */
    public ?object $resolvedcourse = null;

    /**
     * Define the form elements.
     */
    protected function definition(): void {
        $mform = $this->_form;
        $lockedurl = get_config('local_minerva', 'minerva_url');

        $mform->addElement(
            'header',
            'connectionhdr',
            get_string('settings_connection', 'local_minerva')
        );

        if (!empty($lockedurl)) {
            $mform->addElement('hidden', 'minerva_api_url', $lockedurl);
            $mform->setType('minerva_api_url', PARAM_URL);
            $mform->addElement(
                'static',
                'minerva_api_url_display',
                get_string('settings_apiurl', 'local_minerva'),
                s($lockedurl)
            );
        } else {
            $mform->addElement(
                'text',
                'minerva_api_url',
                get_string('settings_apiurl', 'local_minerva'),
                ['size' => 60, 'placeholder' => 'https://minerva.dsv.su.se']
            );
            $mform->setType('minerva_api_url', PARAM_URL);
            $mform->addRule('minerva_api_url', null, 'required', null, 'client');
            $mform->addHelpButton('minerva_api_url', 'settings_apiurl', 'local_minerva');
        }

        $mform->addElement(
            'passwordunmask',
            'minerva_api_key',
            get_string('settings_apikey', 'local_minerva'),
            ['size' => 60]
        );
        $mform->setType('minerva_api_key', PARAM_RAW);
        $mform->addRule('minerva_api_key', null, 'required', null, 'client');

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

        if (!empty($data['minerva_api_url']) && !empty($data['minerva_api_key'])) {
            try {
                $client = new \local_minerva\api_client($data['minerva_api_url'], $data['minerva_api_key']);
                $courses = $client->list_courses();
                if (empty($courses)) {
                    $errors['minerva_api_key'] = get_string('no_scoped_course', 'local_minerva');
                } else {
                    $this->resolvedcourse = reset($courses);
                }
            } catch (\Exception $e) {
                $errors['minerva_api_key'] = get_string(
                    'connection_failed',
                    'local_minerva',
                    $e->getMessage()
                );
            }
        }

        return $errors;
    }
}
