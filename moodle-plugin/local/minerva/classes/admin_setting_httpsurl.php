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
 * Admin URL setting that rejects non-https schemes, except for loopback,
 * host.docker.internal, and bare single-label hostnames (container / LAN).
 *
 * Needed because the API key is sent in the Authorization header; plain http
 * to a public host would leak it.
 *
 * @package    local_minerva
 * @copyright  2026 DSV, Stockholm University
 * @license    http://www.gnu.org/copyleft/gpl.html GNU GPL v3 or later
 */
class admin_setting_httpsurl extends \admin_setting_configtext {
    /**
     * Validate the URL: must parse, and the scheme must be https unless the
     * host is clearly local.
     *
     * @param string $data The submitted value.
     * @return bool|string true on success, error string on failure.
     */
    public function validate($data) {
        $parent = parent::validate($data);
        if ($parent !== true || $data === '') {
            return $parent;
        }
        $parts = parse_url($data);
        if ($parts === false || empty($parts['scheme']) || empty($parts['host'])) {
            return get_string('invalid_api_url', 'local_minerva');
        }
        $scheme = strtolower($parts['scheme']);
        $host = strtolower($parts['host']);
        $islocal = in_array($host, ['localhost', '127.0.0.1', '::1', 'host.docker.internal'], true)
            || strpos($host, '.') === false;
        if ($scheme !== 'https' && !($scheme === 'http' && $islocal)) {
            return get_string('insecure_api_url', 'local_minerva');
        }
        return true;
    }
}
