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

namespace local_minerva;

/**
 * HTTP client for the Minerva integration API.
 *
 * Talks to the /api/integration/* endpoints using the configured API key.
 *
 * @package    local_minerva
 * @copyright  2026 DSV, Stockholm University
 * @license    http://www.gnu.org/copyleft/gpl.html GNU GPL v3 or later
 */
class api_client {
    /** @var string Base URL of the Minerva API. */
    private string $baseurl;

    /** @var string Bearer token for authentication. */
    private string $apikey;

    /**
     * Constructor.
     *
     * @param string $apiurl Minerva API base URL.
     * @param string $apikey Minerva API key.
     * @throws \moodle_exception if credentials are empty.
     */
    public function __construct(string $apiurl, string $apikey) {
        $url = rtrim($apiurl, '/');
        // Accept either instance URL or full API URL.
        if (!str_ends_with($url, '/api')) {
            $url .= '/api';
        }
        $this->baseurl = $url;
        $this->apikey = $apikey;

        if (empty($this->baseurl) || empty($this->apikey)) {
            throw new \moodle_exception('no_api_configured', 'local_minerva');
        }
    }

    /**
     * Create a client from a course link record.
     *
     * @param object $link A local_minerva_links record.
     * @return self
     */
    public static function from_link(object $link): self {
        return new self($link->minerva_api_url, $link->minerva_api_key);
    }

    /**
     * List all active Minerva courses.
     *
     * @return array List of course objects with id, name, description.
     */
    public function list_courses(): array {
        return $this->request('GET', '/integration/courses');
    }

    /**
     * Ensure a user exists in Minerva by eppn.
     *
     * @param string $eppn User's eppn (e.g. user1234@SU.SE).
     * @param string|null $displayname User's display name.
     * @return object User info with id, eppn, created.
     */
    public function ensure_user(string $eppn, ?string $displayname = null): object {
        $body = ['eppn' => $eppn];
        if ($displayname !== null) {
            $body['display_name'] = $displayname;
        }
        return $this->request('POST', '/integration/users/ensure', $body);
    }

    /**
     * Add a user to a Minerva course.
     *
     * @param string $minervacid Minerva course UUID.
     * @param string $eppn User's eppn.
     * @param string|null $displayname User's display name.
     * @param string $role Role in the course (student, teacher, ta).
     * @return object Result with added, user_id.
     */
    public function add_member(
        string $minervacid,
        string $eppn,
        ?string $displayname = null,
        string $role = 'student'
    ): object {
        $body = ['eppn' => $eppn, 'role' => $role];
        if ($displayname !== null) {
            $body['display_name'] = $displayname;
        }
        return $this->request('POST', "/integration/courses/{$minervacid}/members", $body);
    }

    /**
     * Remove a user from a Minerva course.
     *
     * @param string $minervacid Minerva course UUID.
     * @param string $eppn User's eppn.
     * @return object Result with removed boolean.
     */
    public function remove_member(string $minervacid, string $eppn): object {
        $encodedeppn = rawurlencode($eppn);
        return $this->request('DELETE', "/integration/courses/{$minervacid}/members/by-eppn/{$encodedeppn}");
    }

    /**
     * List documents in a Minerva course.
     *
     * @param string $minervacid Minerva course UUID.
     * @return array List of document objects.
     */
    public function list_documents(string $minervacid): array {
        return $this->request('GET', "/integration/courses/{$minervacid}/documents");
    }

    /**
     * Upload a document to a Minerva course.
     *
     * @param string $minervacid Minerva course UUID.
     * @param string $filepath Local path to the file.
     * @param string $filename Original filename.
     * @return object Document info with id, filename, status.
     */
    public function upload_document(string $minervacid, string $filepath, string $filename): object {
        $url = $this->baseurl . "/integration/courses/{$minervacid}/documents";

        $curl = new \curl();
        $curl->setHeader([
            'Authorization: Bearer ' . $this->apikey,
        ]);

        $params = [
            'file' => new \CURLFile($filepath, 'application/pdf', $filename),
        ];

        $response = $curl->post($url, $params);
        $httpcode = $curl->get_info()['http_code'] ?? 0;

        if ($httpcode < 200 || $httpcode >= 300) {
            throw new \moodle_exception(
                'api_error',
                'local_minerva',
                '',
                null,
                "HTTP {$httpcode}: {$response}"
            );
        }

        return json_decode($response);
    }

    /**
     * Create an embed token for a user scoped to a course.
     *
     * @param string $minervacid Minerva course UUID.
     * @param string $eppn User's eppn.
     * @param string|null $displayname User's display name.
     * @return object Token info with token, expires_at.
     */
    public function create_embed_token(string $minervacid, string $eppn, ?string $displayname = null): object {
        $body = ['eppn' => $eppn];
        if ($displayname !== null) {
            $body['display_name'] = $displayname;
        }
        return $this->request('POST', "/integration/courses/{$minervacid}/embed-token", $body);
    }

    /**
     * Make an HTTP request to the Minerva API.
     *
     * @param string $method HTTP method.
     * @param string $path API path (relative to base URL).
     * @param array|null $body Request body (for POST/PUT).
     * @return mixed Decoded JSON response.
     * @throws \moodle_exception on HTTP errors.
     */
    private function request(string $method, string $path, ?array $body = null) {
        $url = $this->baseurl . $path;

        $curl = new \curl();
        $curl->setHeader([
            'Authorization: Bearer ' . $this->apikey,
            'Content-Type: application/json',
            'Accept: application/json',
        ]);

        $options = [];

        switch (strtoupper($method)) {
            case 'GET':
                $response = $curl->get($url, [], $options);
                break;
            case 'POST':
                $response = $curl->post($url, json_encode($body ?? []), $options);
                break;
            case 'DELETE':
                $response = $curl->delete($url, [], $options);
                break;
            default:
                throw new \coding_exception("Unsupported HTTP method: {$method}");
        }

        $httpcode = $curl->get_info()['http_code'] ?? 0;

        if ($httpcode < 200 || $httpcode >= 300) {
            throw new \moodle_exception(
                'api_error',
                'local_minerva',
                '',
                null,
                "HTTP {$httpcode} on {$method} {$path}: {$response}"
            );
        }

        return json_decode($response);
    }
}
