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

namespace local_minerva\task;

use local_minerva\api_client;

/**
 * Scheduled task to sync new course materials to Minerva.
 *
 * Runs periodically to find and upload new resources: stored files from
 * module content areas, URLs, and HTML content from mod_page / mod_book
 * chapters / mod_label / mod_resource intros / section summaries.
 *
 * @package    local_minerva
 * @copyright  2026 DSV, Stockholm University
 * @license    http://www.gnu.org/copyleft/gpl.html GNU GPL v3 or later
 */
class sync_materials extends \core\task\scheduled_task {
    /**
     * Return the task's name.
     *
     * @return string
     */
    public function get_name(): string {
        return get_string('task_sync_materials', 'local_minerva');
    }

    /**
     * Execute the task.
     */
    public function execute(): void {
        global $DB;

        if (!get_config('local_minerva', 'autosync_materials')) {
            mtrace('Minerva materials sync is disabled.');
            return;
        }

        $links = $DB->get_records('local_minerva_links');
        if (empty($links)) {
            mtrace('No Minerva course links found.');
            return;
        }

        foreach ($links as $link) {
            try {
                $client = api_client::from_link($link);
                $this->sync_course($client, $link);
            } catch (\Exception $e) {
                mtrace("  Course {$link->courseid}: API not configured - " . $e->getMessage());
            }
        }
    }

    /**
     * Sync materials for a single linked course.
     *
     * @param api_client $client
     * @param object $link
     */
    private function sync_course(api_client $client, object $link): void {
        $course = get_course($link->courseid);
        if (!$course) {
            mtrace("  Course {$link->courseid} no longer exists, skipping.");
            return;
        }

        $items = self::find_unsynced_resources($course, $link->courseid);

        if (empty($items)) {
            mtrace("  Course {$link->courseid}: no new materials.");
            return;
        }

        $uploaded = self::upload_items($client, $link, $items, function (string $msg): void {
            mtrace('  ' . $msg);
        });

        mtrace("  Course {$link->courseid} -> Minerva {$link->minerva_course_id}: uploaded {$uploaded} new resource(s).");
    }

    /**
     * Upload a list of sync items, recording each successful upload in the
     * sync log. Returns the number uploaded successfully.
     *
     * Each item is a \stdClass with:
     *   - contenthash: string (stable dedup key)
     *   - filename:    string (filename sent to Minerva)
     *   - mimetype:    string
     *   - display:     string (short label for UI)
     *   - sizelabel:   string (optional)
     *   - file:        \stored_file (or null)
     *   - payload:     string (or null; used when file is null)
     *
     * @param api_client $client
     * @param object $link
     * @param \stdClass[] $items
     * @param callable|null $logger fn(string $msg): void for failure messages
     * @return int number of items uploaded
     */
    public static function upload_items(api_client $client, object $link, array $items, ?callable $logger = null): int {
        global $DB;

        $uploaded = 0;

        foreach ($items as $item) {
            $tmpfile = tempnam(sys_get_temp_dir(), 'minerva_');
            if ($item->file instanceof \stored_file) {
                $item->file->copy_content_to($tmpfile);
            } else {
                file_put_contents($tmpfile, $item->payload);
            }

            try {
                $result = $client->upload_document(
                    $link->minerva_course_id,
                    $tmpfile,
                    $item->filename,
                    $item->mimetype
                );

                $record = new \stdClass();
                $record->courseid = $link->courseid;
                $record->contenthash = $item->contenthash;
                $record->filename = $item->filename;
                $record->minerva_doc_id = $result->id ?? '';
                $record->timecreated = time();
                $DB->insert_record('local_minerva_sync_log', $record);

                $uploaded++;
            } catch (\Exception $e) {
                if ($logger) {
                    $logger("Failed to upload {$item->filename}: " . $e->getMessage());
                } else {
                    debugging("Failed to upload {$item->filename}: " . $e->getMessage(), DEBUG_NORMAL);
                }
            } finally {
                @unlink($tmpfile);
            }
        }

        return $uploaded;
    }

    /**
     * Find course resources that haven't been synced yet.
     *
     * Discovers three kinds of sources across visible activities:
     *   1. Stored files in module `content` file areas
     *   2. External URLs from mod_url
     *   3. HTML content: mod_page, mod_book chapters, mod_label intros,
     *      mod_resource intros, and course section summaries
     *
     * Each returned item is a uniform \stdClass (see upload_items()).
     *
     * @param object $course Moodle course object.
     * @param int $courseid Moodle course ID.
     * @return \stdClass[] Unsynced items.
     */
    public static function find_unsynced_resources(object $course, int $courseid): array {
        global $DB;

        $modinfo = get_fast_modinfo($course);
        $fs = get_file_storage();
        $items = [];

        foreach ($modinfo->get_cms() as $cm) {
            if (!$cm->visible || !$cm->available) {
                continue;
            }

            $modcontext = \context_module::instance($cm->id);

            if ($cm->modname === 'url') {
                $urlrecord = $DB->get_record('url', ['id' => $cm->instance], 'id, externalurl, name');
                if ($urlrecord && !empty($urlrecord->externalurl)) {
                    $items[] = self::build_url_item($urlrecord, $cm);
                }
                continue;
            }

            if ($cm->modname === 'page') {
                $pagerec = $DB->get_record(
                    'page',
                    ['id' => $cm->instance],
                    'id, name, content, contentformat'
                );
                if ($pagerec) {
                    $item = self::build_html_item(
                        'page',
                        (int) $pagerec->id,
                        $pagerec->name ?: $cm->name,
                        $pagerec->content,
                        (int) $pagerec->contentformat,
                        $modcontext
                    );
                    if ($item) {
                        $items[] = $item;
                    }
                }
                continue;
            }

            if ($cm->modname === 'book') {
                $bookrec = $DB->get_record('book', ['id' => $cm->instance], 'id, name');
                if ($bookrec) {
                    $chapters = $DB->get_records(
                        'book_chapters',
                        ['bookid' => $bookrec->id, 'hidden' => 0],
                        'pagenum ASC',
                        'id, title, content, contentformat'
                    );
                    foreach ($chapters as $chapter) {
                        $label = ($bookrec->name ?: $cm->name) . ' / ' . $chapter->title;
                        $item = self::build_html_item(
                            'book_chapter',
                            (int) $chapter->id,
                            $label,
                            $chapter->content,
                            (int) $chapter->contentformat,
                            $modcontext
                        );
                        if ($item) {
                            $items[] = $item;
                        }
                    }
                }
                continue;
            }

            if ($cm->modname === 'label') {
                $labelrec = $DB->get_record(
                    'label',
                    ['id' => $cm->instance],
                    'id, intro, introformat'
                );
                if ($labelrec) {
                    $item = self::build_html_item(
                        'label',
                        (int) $labelrec->id,
                        $cm->name,
                        $labelrec->intro,
                        (int) $labelrec->introformat,
                        $modcontext
                    );
                    if ($item) {
                        $items[] = $item;
                    }
                }
                // Labels have no file area; nothing else to collect.
                continue;
            }

            if ($cm->modname === 'resource') {
                $resrec = $DB->get_record(
                    'resource',
                    ['id' => $cm->instance],
                    'id, name, intro, introformat'
                );
                if ($resrec) {
                    $item = self::build_html_item(
                        'resource_intro',
                        (int) $resrec->id,
                        ($resrec->name ?: $cm->name) . ' (description)',
                        $resrec->intro,
                        (int) $resrec->introformat,
                        $modcontext
                    );
                    if ($item) {
                        $items[] = $item;
                    }
                }
                self::collect_module_files($fs, $modcontext, $cm, $items);
                continue;
            }

            self::collect_module_files($fs, $modcontext, $cm, $items);
        }

        // Section summaries (includes the top "general" section 0 and any
        // visible named sections the teacher has written).
        $coursecontext = \context_course::instance($courseid);
        foreach ($modinfo->get_section_info_all() as $section) {
            if (empty($section->visible)) {
                continue;
            }
            if (empty($section->summary)) {
                continue;
            }
            $label = trim((string) ($section->name ?? ''));
            if ($label === '') {
                $label = 'Section ' . $section->section;
            }
            $item = self::build_html_item(
                'section',
                (int) $section->id,
                $label . ' (section summary)',
                $section->summary,
                (int) ($section->summaryformat ?? FORMAT_HTML),
                $coursecontext
            );
            if ($item) {
                $items[] = $item;
            }
        }

        // Filter out items already synced (by content hash).
        $alreadysynced = $DB->get_records_menu(
            'local_minerva_sync_log',
            ['courseid' => $courseid],
            '',
            'contenthash, id'
        );

        $fresh = [];
        foreach ($items as $item) {
            if (!isset($alreadysynced[$item->contenthash])) {
                $fresh[] = $item;
            }
        }

        return $fresh;
    }

    /**
     * Collect all non-directory files from a module's `content` file area.
     *
     * @param \file_storage $fs
     * @param \context $modcontext
     * @param \cm_info $cm
     * @param \stdClass[] &$items
     */
    private static function collect_module_files(
        \file_storage $fs,
        \context $modcontext,
        \cm_info $cm,
        array &$items
    ): void {
        $component = 'mod_' . $cm->modname;
        $files = $fs->get_area_files(
            $modcontext->id,
            $component,
            'content',
            false,
            'filename',
            false
        );
        foreach ($files as $file) {
            if ($file->is_directory()) {
                continue;
            }
            $item = new \stdClass();
            $item->contenthash = $file->get_contenthash();
            $item->filename = $file->get_filename();
            $item->mimetype = $file->get_mimetype() ?: 'application/octet-stream';
            $item->display = $file->get_filename();
            $item->sizelabel = display_size($file->get_filesize());
            $item->file = $file;
            $item->payload = null;
            $items[] = $item;
        }
    }

    /**
     * Build a sync item for an external URL module.
     *
     * @param object $urlrecord
     * @param \cm_info $cm
     * @return \stdClass
     */
    private static function build_url_item(object $urlrecord, \cm_info $cm): \stdClass {
        $name = $urlrecord->name ?: $cm->name;
        $filename = self::safe_slug($name) . '.url';

        $item = new \stdClass();
        $item->contenthash = sha1($urlrecord->externalurl);
        $item->filename = $filename;
        $item->mimetype = 'text/x-url';
        $item->display = $name . ' (URL)';
        $item->sizelabel = '';
        $item->file = null;
        $item->payload = $urlrecord->externalurl;
        return $item;
    }

    /**
     * Build a sync item from a Moodle HTML text field. Returns null if the
     * field has no meaningful content.
     *
     * @param string $type     Stable type key ("page", "book_chapter", etc.)
     * @param int    $instanceid Stable instance ID within the type.
     * @param string $title    Human-readable title for filename + display.
     * @param string|null $content Raw HTML/text content from Moodle.
     * @param int    $format   Moodle FORMAT_* constant.
     * @param \context $context Context used for format_text().
     * @return \stdClass|null
     */
    private static function build_html_item(
        string $type,
        int $instanceid,
        string $title,
        ?string $content,
        int $format,
        \context $context
    ): ?\stdClass {
        if ($content === null || trim(strip_tags($content)) === '') {
            return null;
        }

        // Normalise whatever format Moodle stored into real HTML.
        $html = format_text($content, $format, [
            'context' => $context,
            'noclean' => true,
            'filter' => false,
            'para' => false,
        ]);

        $document = "<!DOCTYPE html>\n<html><head><meta charset=\"utf-8\"><title>"
            . htmlspecialchars($title, ENT_QUOTES, 'UTF-8')
            . "</title></head><body>\n"
            . '<h1>' . htmlspecialchars($title, ENT_QUOTES, 'UTF-8') . "</h1>\n"
            . $html
            . "\n</body></html>\n";

        $filename = self::safe_slug($type . '-' . $title) . '.html';

        $item = new \stdClass();
        $item->contenthash = sha1($type . ':' . $instanceid . ':' . sha1($document));
        $item->filename = $filename;
        $item->mimetype = 'text/html';
        $item->display = $title;
        $item->sizelabel = display_size(strlen($document));
        $item->file = null;
        $item->payload = $document;
        return $item;
    }

    /**
     * Turn a free-text title into a safe, bounded filename slug.
     *
     * @param string $name
     * @return string
     */
    private static function safe_slug(string $name): string {
        $slug = preg_replace('/[^a-zA-Z0-9_\-]+/', '_', $name);
        $slug = trim($slug, '_');
        if ($slug === '') {
            $slug = 'untitled';
        }
        if (strlen($slug) > 120) {
            $slug = substr($slug, 0, 120);
        }
        return $slug;
    }
}
