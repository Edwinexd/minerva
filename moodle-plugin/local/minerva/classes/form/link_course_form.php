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
 * Two modes, selected automatically based on plugin settings:
 *
 *   Site-integration mode (admin has configured `site_api_key`):
 *     The form shows a dropdown of Minerva courses the current teacher
 *     owns or teaches, populated from /api/integration/site/courses-for-user.
 *     Picking one and submitting makes the plugin call
 *     /api/integration/site/provision, which mints and returns a regular
 *     per-course api_key. Caller stores it in local_minerva_links.
 *
 *   Legacy mode (site key not set):
 *     Teacher pastes a course-scoped api_key they created manually in
 *     Minerva. The form calls list_courses() during validation to
 *     resolve which course the key is scoped to.
 *
 * @package    local_minerva
 * @copyright  2026 DSV, Stockholm University
 * @license    http://www.gnu.org/copyleft/gpl.html GNU GPL v3 or later
 */
class link_course_form extends \moodleform {
    /**
     * Scoped course resolved during validation.
     *
     * Legacy mode: populated from `api_client::list_courses()` against
     * the pasted key.
     * Site-integration mode: populated from the picker's selected course
     * (so manage.php has a name + id to persist without a second API call).
     *
     * @var object|null
     */
    public ?object $resolvedcourse = null;

    /**
     * Newly-minted per-course key when using site-integration mode.
     * Null in legacy mode; caller reads `minerva_api_key` from form data instead.
     *
     * @var string|null
     */
    public ?string $provisionedkey = null;

    /**
     * Cached site-integration client + course list. Populated lazily via
     * `get_site_courses()` and reused between `definition()` (to render
     * the picker) and `validation()` (to look up the chosen option).
     *
     * @var array|null
     */
    private ?array $sitecourses = null;

    /**
     * Site-integration-mode error surfaced on the form; e.g. the teacher's
     * eppn isn't known to Minerva yet, or the site key is invalid.
     *
     * @var string|null
     */
    private ?string $siteerror = null;

    /**
     * Whether the caller's user profile couldn't be found in Minerva. Shown
     * as an inline notice instead of failing the whole page load.
     *
     * @var bool
     */
    private bool $siteuserunknown = false;

    /**
     * Define the form elements.
     */
    protected function definition(): void {
        global $USER;

        $mform = $this->_form;
        $lockedurl = get_config('local_minerva', 'minerva_url');
        $usesite = \local_minerva\api_client::site_integration_available();

        $mform->addElement(
            'header',
            'connectionhdr',
            get_string('settings_connection', 'local_minerva')
        );

        if ($usesite) {
            // Site integration mode: URL is always locked, no per-course key.
            $mform->addElement('hidden', 'minerva_api_url', $lockedurl);
            $mform->setType('minerva_api_url', PARAM_URL);
            $mform->addElement(
                'static',
                'minerva_api_url_display',
                get_string('settings_apiurl', 'local_minerva'),
                s($lockedurl)
            );

            $courses = $this->get_site_courses($USER);
            if ($this->siteerror !== null) {
                $mform->addElement(
                    'static',
                    'sitekey_error',
                    '',
                    \html_writer::div(
                        s($this->siteerror),
                        'alert alert-danger'
                    )
                );
            } else if ($this->siteuserunknown) {
                $mform->addElement(
                    'static',
                    'sitekey_user_unknown',
                    '',
                    \html_writer::div(
                        get_string('site_user_not_found', 'local_minerva'),
                        'alert alert-warning'
                    )
                );
            }

            $options = ['' => get_string('select_minerva_course', 'local_minerva')];
            foreach ($courses as $c) {
                $options[$c->id] = $c->name;
            }
            $mform->addElement(
                'select',
                'minerva_course_id',
                get_string('select_minerva_course', 'local_minerva'),
                $options
            );
            $mform->setType('minerva_course_id', PARAM_RAW);
            $mform->addRule('minerva_course_id', null, 'required', null, 'client');

            if ($this->siteerror === null && !$this->siteuserunknown && empty($courses)) {
                $mform->addElement(
                    'static',
                    'no_teachable_courses',
                    '',
                    \html_writer::div(
                        get_string('site_no_teachable_courses', 'local_minerva'),
                        'alert alert-info'
                    )
                );
            }
        } else if (!empty($lockedurl)) {
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

        if (!$usesite) {
            $mform->addElement(
                'passwordunmask',
                'minerva_api_key',
                get_string('settings_apikey', 'local_minerva'),
                ['size' => 60]
            );
            $mform->setType('minerva_api_key', PARAM_RAW);
            $mform->addRule('minerva_api_key', null, 'required', null, 'client');
        }

        $mform->addElement('hidden', 'courseid');
        $mform->setType('courseid', PARAM_INT);

        $this->add_action_buttons(true, get_string('link_course', 'local_minerva'));
    }

    /**
     * Look up the picker options by calling /integration/site/courses-for-user
     * once per form render. Caches the result + the resolved api_client so
     * validation can reuse them without a second network hop.
     *
     * @param \stdClass $user
     * @return array List of course objects (id, name, description).
     */
    private function get_site_courses(\stdClass $user): array {
        if ($this->sitecourses !== null) {
            return $this->sitecourses;
        }
        try {
            $client = \local_minerva\api_client::from_site_config();
            $resp = $client->site_courses_for_user($user->username);
            if (!empty($resp->user_exists)) {
                $this->sitecourses = $resp->courses ?? [];
            } else {
                $this->siteuserunknown = true;
                $this->sitecourses = [];
            }
        } catch (\Exception $e) {
            $this->siteerror = get_string('connection_failed', 'local_minerva', $e->getMessage());
            $this->sitecourses = [];
        }
        return $this->sitecourses;
    }

    /**
     * Validate the form data.
     *
     * @param array $data
     * @param array $files
     * @return array Errors keyed by field name.
     */
    public function validation($data, $files): array {
        global $USER;

        $errors = parent::validation($data, $files);
        $usesite = \local_minerva\api_client::site_integration_available();

        if ($usesite) {
            if ($this->siteerror !== null) {
                $errors['minerva_course_id'] = $this->siteerror;
                return $errors;
            }
            $selected = $data['minerva_course_id'] ?? '';
            $match = null;
            foreach ($this->get_site_courses($USER) as $c) {
                if ($c->id === $selected) {
                    $match = $c;
                    break;
                }
            }
            if ($match === null) {
                $errors['minerva_course_id'] = get_string('site_course_not_selectable', 'local_minerva');
                return $errors;
            }
            // Provision now so that if the backend rejects (e.g. ACL race),
            // the teacher sees it on the same submission rather than a 500
            // after we've stored a half-baked link row.
            try {
                $client = \local_minerva\api_client::from_site_config();
                $courseobj = \get_course((int) ($data['courseid'] ?? 0));
                $keyname = format_string($courseobj->fullname);
                $minted = $client->site_provision_course_key(
                    $USER->username,
                    $keyname,
                    $match->id
                );
                $this->resolvedcourse = $match;
                $this->provisionedkey = $minted->key ?? null;
                if (empty($this->provisionedkey)) {
                    $errors['minerva_course_id'] = get_string('site_provision_empty_key', 'local_minerva');
                }
            } catch (\Exception $e) {
                $errors['minerva_course_id'] = get_string(
                    'connection_failed',
                    'local_minerva',
                    $e->getMessage()
                );
            }
            return $errors;
        }

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
