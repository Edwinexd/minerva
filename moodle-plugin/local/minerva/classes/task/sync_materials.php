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

        // Discover everything currently on the Moodle side. We need
        // BOTH the unsynced subset (to upload) AND the full set (to
        // build the reconcile sweep's source_refs list); compute the
        // full set first, then filter.
        $allitems = self::find_all_resources($course, $link->courseid);
        $items = self::filter_unsynced($link->courseid, $allitems);

        if (!empty($items)) {
            $uploaded = self::upload_items($client, $link, $items, function (string $msg): void {
                mtrace('  ' . $msg);
            });
            mtrace("  Course {$link->courseid} -> Minerva {$link->minerva_course_id}: uploaded {$uploaded} new resource(s).");
        } else {
            mtrace("  Course {$link->courseid}: no new materials.");
        }

        // Reconcile sweep: tell Minerva the full set of source_refs
        // we still see. Anything the server has under
        // (course, source_system='moodle') that's not in the list
        // gets orphaned and excluded from new retrievals. Catches
        // everything the observer missed (bulk delete, plugin-disabled
        // gaps, restore-from-backup) and runs even when no new items
        // were uploaded this round.
        try {
            $currentrefs = self::current_source_refs($link->courseid, $allitems);
            $orphaned = $client->reconcile_source_refs($link->minerva_course_id, $currentrefs);
            if (!empty($orphaned)) {
                mtrace(
                    "  Course {$link->courseid}: reconcile orphaned " . count($orphaned) . ' stale doc(s).'
                );
            }
        } catch (\Exception $e) {
            mtrace("  Course {$link->courseid}: reconcile failed: " . $e->getMessage());
        }
    }

    /**
     * Upload a list of sync items, recording each successful upload in the
     * sync log. Returns the number uploaded successfully.
     *
     * Each item is a \stdClass with:
     *   - contenthash: string (stable client-side dedup key, optimisation only)
     *   - sourceref:   string (Moodle origin identity sent to server as source_ref)
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
        $seenhashes = [];

        foreach ($items as $item) {
            // Skip duplicates within this batch (e.g. two labels with the same content):
            // the sync_log has UNIQUE(courseid, contenthash) and would crash on insert.
            if (isset($seenhashes[$item->contenthash])) {
                continue;
            }
            $seenhashes[$item->contenthash] = true;

            $tmpfile = tempnam(sys_get_temp_dir(), 'minerva_');
            if ($tmpfile === false) {
                $msg = "Failed to allocate temp file for {$item->filename}";
                if ($logger) {
                    $logger($msg);
                } else {
                    debugging($msg, DEBUG_NORMAL);
                }
                continue;
            }

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
                    $item->mimetype,
                    $item->sourceref ?? null
                );

                $record = new \stdClass();
                $record->courseid = $link->courseid;
                $record->contenthash = $item->contenthash;
                $record->sourceref = $item->sourceref ?? null;
                $record->filename = $item->filename;
                $record->minerva_doc_id = $result->id ?? '';
                $record->timecreated = time();
                try {
                    $DB->insert_record('local_minerva_sync_log', $record);
                } catch (\dml_exception $de) {
                    // Concurrent run inserted the same (courseid, contenthash) row first.
                    // The upload to Minerva already succeeded; treat as no-op.
                    debugging("sync_log insert raced for {$item->filename}: " . $de->getMessage(), DEBUG_DEVELOPER);
                }

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
     * Collect every source_ref the plugin currently considers "present"
     * for this course. Includes both items already uploaded in past
     * runs (from local_minerva_sync_log) AND items discovered this run
     * but not yet uploaded. Excludes items whose sourceref is null
     * (legacy sync_log rows from before slice 2 rolled out; those are
     * reconciled out over time as the corresponding Moodle objects
     * get edited or re-discovered).
     *
     * Used by sync_course() to post a reconcile sweep to Minerva at
     * the end of each run, orphaning anything in Minerva whose Moodle
     * source has disappeared since the last sync.
     *
     * @param int $courseid Moodle course id.
     * @param \stdClass[] $discovered Items the current sync found (with sourceref).
     * @return string[] Deduped list of currently-known source_refs.
     */
    public static function current_source_refs(int $courseid, array $discovered): array {
        global $DB;

        $refs = [];
        // Discovered this run.
        foreach ($discovered as $item) {
            if (!empty($item->sourceref)) {
                $refs[$item->sourceref] = true;
            }
        }
        // Previously uploaded (covers items that weren't re-discovered
        // this run because they didn't actually change ; reconcile only
        // orphans missing-from-the-list, so an unchanged item that's
        // still in sync_log must appear).
        $logrefs = $DB->get_fieldset_select(
            'local_minerva_sync_log',
            'sourceref',
            'courseid = :courseid AND sourceref IS NOT NULL AND sourceref <> :empty',
            ['courseid' => $courseid, 'empty' => '']
        );
        foreach ($logrefs as $r) {
            $refs[$r] = true;
        }
        return array_keys($refs);
    }

    /**
     * Discover every Moodle resource the plugin currently considers
     * syncable, regardless of whether it has been uploaded before.
     * Each item gets a stable `sourceref` so the slice-2 server side
     * can do orphan-on-replace + reconcile.
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
     * @return \stdClass[] All current items, deduped.
     */
    public static function find_all_resources(object $course, int $courseid): array {
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
                        $modcontext,
                        (int) $cm->id
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
                            $modcontext,
                            (int) $cm->id
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
                        $modcontext,
                        (int) $cm->id
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
                        $modcontext,
                        (int) $cm->id
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
                $coursecontext,
                null
            );
            if ($item) {
                $items[] = $item;
            }
        }

        return $items;
    }

    /**
     * Filter out items already uploaded in a previous sync (matched by
     * client-side contenthash). Client-side filtering is now an
     * optimisation only ; the server enforces dedup authoritatively
     * via the (course, content_hash) partial unique index. Without
     * this filter the plugin would re-POST every item every run; the
     * server would dedup correctly but we'd burn one HTTP request per
     * item per cron tick.
     *
     * @param int $courseid Moodle course ID.
     * @param \stdClass[] $items All current items from find_all_resources().
     * @return \stdClass[] Items the local sync log doesn't yet know about.
     */
    public static function filter_unsynced(int $courseid, array $items): array {
        global $DB;

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
     * Backwards-compatible shim for the manual sync.php UI which still
     * wants "what's new". Equivalent to find_all_resources +
     * filter_unsynced.
     *
     * @param object $course
     * @param int $courseid
     * @return \stdClass[]
     */
    public static function find_unsynced_resources(object $course, int $courseid): array {
        return self::filter_unsynced($courseid, self::find_all_resources($course, $courseid));
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
            // Source identity for the slice-2 server: cm + filepath +
            // filename. Stable across file replacements (Moodle keeps
            // the path/name the same when a teacher swaps the bytes
            // behind a resource), so a replaced file flips the
            // server's content_hash branch and orphans the old row
            // with the matching source_ref instead of creating an
            // orphan + new pair.
            $item->sourceref = sprintf(
                'mod_file:cm:%d:%s%s',
                $cm->id,
                $file->get_filepath(),
                $file->get_filename()
            );
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
        // Source identity ties to the cm, so a teacher editing the URL
        // (different externalurl, same cm) is treated as a content
        // change and the previous Minerva doc is orphaned via the
        // source_ref branch in upload_or_dedup.
        $item->sourceref = 'url:cm:' . $cm->id;
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
        \context $context,
        ?int $cmid
    ): ?\stdClass {
        if ($content === null || trim(strip_tags($content)) === '') {
            return null;
        }

        // Normalise whatever format Moodle stored into real HTML. Run through
        // HTML Purifier (noclean=false) to strip scripts and other unsafe markup
        // before we ship the payload off to Minerva. No Moodle filters --
        // want the raw authored content, not filter-expanded output.
        $html = format_text($content, $format, [
            'context' => $context,
            'noclean' => false,
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
        // Source identity prefers cm-scoped refs when the item maps
        // to a specific course module (pages, labels, book chapters,
        // resource intros all do); section summaries have no cm and
        // fall back to (type, instanceid). Editing the underlying
        // Moodle object keeps the same source_ref but changes the
        // content_hash; the server orphans the previous doc and
        // takes the new one as the active version for that slot.
        if ($cmid !== null && $type !== 'book_chapter') {
            $item->sourceref = $type . ':cm:' . $cmid;
        } else {
            // Book chapters: each chapter is its own unit even within
            // one cm. Section summaries: keyed by section id (the
            // course-level identity, not the cm).
            $item->sourceref = $type . ':' . $instanceid;
        }
        return $item;
    }

    /**
     * Turn a free-text title into a safe, bounded filename slug.
     *
     * @param string $name
     * @return string
     */
    private static function safe_slug(string $name): string {
        // Keep letters (any script) and digits; collapse everything else to '_'.
        $slug = preg_replace('/[^\p{L}\p{N}_\-]+/u', '_', $name);
        $slug = trim($slug, '_');
        if ($slug === '' || $slug === null) {
            $slug = 'untitled';
        }
        if (function_exists('mb_strlen') && mb_strlen($slug, 'UTF-8') > 120) {
            $slug = mb_substr($slug, 0, 120, 'UTF-8');
        } else if (strlen($slug) > 120) {
            $slug = substr($slug, 0, 120);
        }
        return $slug;
    }
}
