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
 * Discover mod_forum content and serialise it for upload to Minerva.
 *
 * Privacy posture (slice 3):
 *   - Two-level opt-in: site-level `enable_forum_sync` admin setting
 *     AND per-link `sync_forums` teacher toggle. Both default off
 *     for new installations (the site-level is shipped ON in
 *     settings.php so admins don't have to touch it, but operators
 *     can flip it to OFF for compliance and it's then a hard kill
 *     switch).
 *   - Only threads with at least one teacher reply are kept ("teacher
 *     posted" = a user with editingteacher OR teacher role in the
 *     course context posted in the discussion).
 *   - Student names from the course roster (firstname, lastname,
 *     username local-part for `ext:`-style usernames) are scrubbed
 *     from all post bodies before serialisation. Best-effort literal
 *     replacement with `[student]`; not a guarantee against every
 *     PII shape but a strong default for the common cases.
 *
 * Source identity: one Minerva doc per forum, `source_ref =
 * forum:{forum_id}`. Re-uploading after content changes triggers
 * the slice-2 orphan-on-replace path so retrieval stays current.
 *
 * @package    local_minerva
 * @copyright  2026 DSV, Stockholm University
 * @license    http://www.gnu.org/copyleft/gpl.html GNU GPL v3 or later
 */
class forum_collector {
    /**
     * Whether forum sync should run for this link. AND of the
     * site-level admin setting and the per-link teacher toggle.
     * Plugin code that wants to gate on forum support must call this
     * (don't read either flag directly).
     *
     * @param object $link Row from local_minerva_links.
     * @return bool
     */
    public static function should_sync(object $link): bool {
        if (!get_config('local_minerva', 'enable_forum_sync')) {
            return false;
        }
        return !empty($link->sync_forums);
    }

    /**
     * Build the upload items for every kept forum in the course.
     * Returns an array of \stdClass items shaped exactly like the
     * ones produced by sync_materials::find_all_resources so the
     * existing upload_items() + reconcile pipeline handles them
     * unchanged.
     *
     * @param object $course Moodle course object.
     * @param int $courseid Moodle course id.
     * @return \stdClass[] Forum items ready for upload.
     */
    public static function collect(object $course, int $courseid): array {
        global $DB, $CFG;

        require_once($CFG->dirroot . '/mod/forum/lib.php');

        // Enumerate forum instances in the course.
        $coursemodinfo = get_fast_modinfo($course);
        $forumcms = $coursemodinfo->get_instances_of('forum');
        if (empty($forumcms)) {
            return [];
        }

        // Compute the teacher-userid set once per course; reused for
        // every discussion to test "did a teacher post here?".
        $teacherids = self::teacher_user_ids($courseid);

        // Compute the PII denylist once per course; reused per post.
        $studentnames = self::student_name_denylist($courseid, $teacherids);

        $items = [];

        foreach ($forumcms as $cm) {
            if (!$cm->visible || !$cm->available) {
                continue;
            }
            $forumrec = $DB->get_record('forum', ['id' => $cm->instance], 'id, name, intro, type');
            if (!$forumrec) {
                continue;
            }

            // Get all discussions in this forum.
            $discussions = $DB->get_records(
                'forum_discussions',
                ['forum' => $forumrec->id],
                'timemodified DESC',
                'id, name, userid, timemodified, firstpost'
            );
            if (empty($discussions)) {
                continue;
            }

            $keptthreads = [];
            foreach ($discussions as $disc) {
                $posts = $DB->get_records(
                    'forum_posts',
                    ['discussion' => $disc->id],
                    'created ASC',
                    'id, parent, userid, subject, message, messageformat, created'
                );
                if (empty($posts)) {
                    continue;
                }
                // Slice-3 contract: only keep threads where a teacher
                // has posted at least once. Pure student-to-student
                // exchanges are excluded regardless of how lively
                // they are.
                $teacherpresent = false;
                foreach ($posts as $p) {
                    if (isset($teacherids[(int) $p->userid])) {
                        $teacherpresent = true;
                        break;
                    }
                }
                if (!$teacherpresent) {
                    continue;
                }
                $keptthreads[] = (object) [
                    'discussion' => $disc,
                    'posts' => $posts,
                ];
            }

            if (empty($keptthreads)) {
                // Skip emitting an empty doc ; the reconcile sweep
                // would orphan it on the next run anyway and we'd
                // just churn the upload pipeline for no gain.
                continue;
            }

            $items[] = self::build_forum_item(
                $forumrec,
                $cm,
                $keptthreads,
                $teacherids,
                $studentnames
            );
        }

        return $items;
    }

    /**
     * User IDs in the course context that hold a teacher role
     * (editingteacher or teacher). Used to test "did a teacher post
     * here?" per discussion and to exclude teachers from the PII
     * denylist (we explicitly DO want teachers' names visible in
     * Minerva so students can see who answered).
     *
     * @param int $courseid
     * @return array<int, true> Map of userid -> true for set membership.
     */
    private static function teacher_user_ids(int $courseid): array {
        global $DB;

        $coursecontext = \context_course::instance($courseid);
        // Role shortnames: 'editingteacher' is the standard
        // teacher-with-edit role; 'teacher' is the non-editing
        // teacher (TA-like). Both count for slice 3 per user spec.
        $roles = $DB->get_records_select(
            'role',
            'shortname IN (:r1, :r2)',
            ['r1' => 'editingteacher', 'r2' => 'teacher'],
            '',
            'id, shortname'
        );
        if (empty($roles)) {
            return [];
        }
        $roleids = array_keys($roles);
        [$insql, $inparams] = $DB->get_in_or_equal($roleids, SQL_PARAMS_NAMED, 'r');
        $params = array_merge(['ctxid' => $coursecontext->id], $inparams);
        $rows = $DB->get_fieldset_select(
            'role_assignments',
            'userid',
            "contextid = :ctxid AND roleid {$insql}",
            $params
        );
        $set = [];
        foreach ($rows as $uid) {
            $set[(int) $uid] = true;
        }
        return $set;
    }

    /**
     * Build the PII-scrub denylist from the course roster: firstname,
     * lastname, and the username's local-part for `ext:`-style
     * external users. Excludes teachers (we want their names visible
     * in the indexed corpus) and applies a minimum-length floor of 3
     * characters so absurdly short names ("Li") don't scrub every
     * occurrence of those letters across all post bodies.
     *
     * Returned list is sorted longest-first so a regex alternation
     * preferentially matches the longest available token (catches
     * "Anna-Maria" before "Anna" if both appear).
     *
     * @param int $courseid
     * @param array<int, true> $teacherids
     * @return string[] Lowercased, deduped, sorted-by-length-desc.
     */
    public static function student_name_denylist(int $courseid, array $teacherids): array {
        global $DB;

        $coursecontext = \context_course::instance($courseid);
        // Enrolled users in the course. We can't filter on student
        // role specifically (some courses use custom roles, role
        // semantics drift across installations), so the rule is
        // "everyone enrolled who isn't a teacher".
        $users = get_enrolled_users($coursecontext, '', 0, 'u.id, u.firstname, u.lastname, u.username');

        $names = [];
        foreach ($users as $u) {
            if (isset($teacherids[(int) $u->id])) {
                continue;
            }
            foreach ([$u->firstname, $u->lastname] as $n) {
                $n = trim((string) $n);
                if ($n !== '' && self::ucs2_len($n) >= 3) {
                    $names[mb_strtolower($n, 'UTF-8')] = true;
                }
            }
            // Local-part of username for ext:-style external accounts
            // (those use the `ext:` eppn prefix scheme; the part after
            // `:` or `@` is often a real name fragment).
            $ulocalpart = preg_split('/[:@]/', (string) $u->username)[0] ?? '';
            $ulocalpart = trim($ulocalpart);
            if ($ulocalpart !== '' && self::ucs2_len($ulocalpart) >= 3) {
                $names[mb_strtolower($ulocalpart, 'UTF-8')] = true;
            }
        }
        $list = array_keys($names);
        usort($list, function ($a, $b) {
            return self::ucs2_len($b) <=> self::ucs2_len($a);
        });
        return $list;
    }

    /**
     * Length in code-points (not bytes). PHP's strlen counts bytes,
     * which would let "Åsa" (3 code-points, 4 bytes UTF-8) clear a
     * `>= 3` byte threshold while being only 3 visible characters ;
     * counting code-points keeps the privacy floor consistent across
     * Latin-1 vs non-ASCII names.
     *
     * @param string $s
     * @return int
     */
    private static function ucs2_len(string $s): int {
        if (function_exists('mb_strlen')) {
            return mb_strlen($s, 'UTF-8');
        }
        return strlen($s);
    }

    /**
     * Replace every word-boundary case-insensitive match of any name
     * in `$denylist` with `[student]`. The denylist must be sorted
     * longest-first so PCRE's alternation picks the longest match
     * (it's leftmost-first, not leftmost-longest); the caller does
     * this in `student_name_denylist`.
     *
     * Operates on the raw post HTML/text, before format_text(); we
     * scrub the source rather than the rendered output so the
     * downstream HTML rendering doesn't reintroduce names from
     * filter expansions (e.g. an `@mention` filter that resolves
     * to "Posted by Jane Smith").
     *
     * @param string|null $text
     * @param string[] $denylist Names, lowercased, longest-first.
     * @return string Scrubbed text (empty string when input is null).
     */
    public static function scrub_pii(?string $text, array $denylist): string {
        if ($text === null || $text === '') {
            return '';
        }
        if (empty($denylist)) {
            return $text;
        }
        // Escape each name for regex and join with alternation.
        $alts = array_map(function ($n) {
            return preg_quote($n, '/');
        }, $denylist);
        // Word boundaries via Unicode-aware lookarounds: \b alone
        // uses ASCII word boundaries, which mis-detects "word" for
        // non-ASCII names. The `\p{L}` class covers Unicode letters,
        // which is what we actually mean by "word" here.
        $pattern = '/(?<![\p{L}\p{N}_])(' . implode('|', $alts) . ')(?![\p{L}\p{N}_])/iu';
        return preg_replace($pattern, '[student]', $text);
    }

    /**
     * Serialise the kept threads of one forum into a single HTML
     * document. Each thread renders as a section with the
     * discussion title and the posts in chronological order. Author
     * lines distinguish "Teacher" vs "Student" (no real names from
     * either side ; teacher names are preserved as a courtesy in
     * the body if they appear, but the author byline is normalised
     * to keep the corpus uniform).
     *
     * @param object $forumrec Forum record.
     * @param \cm_info $cm
     * @param array $keptthreads [{discussion, posts}]
     * @param array<int, true> $teacherids
     * @param string[] $denylist
     * @return \stdClass Item ready for sync_materials::upload_items.
     */
    private static function build_forum_item(
        object $forumrec,
        \cm_info $cm,
        array $keptthreads,
        array $teacherids,
        array $denylist
    ): \stdClass {
        $modcontext = \context_module::instance($cm->id);

        $forumtitle = $forumrec->name ?: $cm->name;
        $body = '<h1>' . htmlspecialchars($forumtitle, ENT_QUOTES, 'UTF-8') . "</h1>\n";

        foreach ($keptthreads as $thread) {
            $disc = $thread->discussion;
            // Discussion subject can itself contain PII (Moodle lets
            // users put anything in the title); scrub before render.
            $disctitle = self::scrub_pii((string) $disc->name, $denylist);
            $body .= "<section>\n";
            $body .= '<h2>' . htmlspecialchars($disctitle, ENT_QUOTES, 'UTF-8') . "</h2>\n";
            foreach ($thread->posts as $p) {
                $isteacher = isset($teacherids[(int) $p->userid]);
                $authorlabel = $isteacher ? 'Teacher' : 'Student';
                // Scrub PII out of the source markup, then run through
                // format_text+purifier to produce safe HTML. No
                // Moodle filters (same posture as sync_materials'
                // build_html_item): we want the raw authored content,
                // not filter-expanded mentions or autolinks.
                $scrubbed = self::scrub_pii((string) $p->message, $denylist);
                $rendered = format_text(
                    $scrubbed,
                    (int) $p->messageformat,
                    [
                        'context' => $modcontext,
                        'noclean' => false,
                        'filter' => false,
                        'para' => false,
                    ]
                );
                $subject = self::scrub_pii((string) ($p->subject ?? ''), $denylist);
                $body .= "<article>\n";
                $body .= '<header><strong>' . htmlspecialchars($authorlabel, ENT_QUOTES, 'UTF-8') . '</strong>';
                if ($subject !== '' && $subject !== $disctitle) {
                    $body .= ' &mdash; <em>' . htmlspecialchars($subject, ENT_QUOTES, 'UTF-8') . '</em>';
                }
                $body .= "</header>\n";
                $body .= "<div>{$rendered}</div>\n";
                $body .= "</article>\n";
            }
            $body .= "</section>\n";
        }

        $document = "<!DOCTYPE html>\n<html><head><meta charset=\"utf-8\"><title>"
            . htmlspecialchars($forumtitle, ENT_QUOTES, 'UTF-8')
            . "</title></head><body>\n"
            . $body
            . "\n</body></html>\n";

        $filename = self::safe_slug('forum-' . $forumtitle) . '.html';

        $item = new \stdClass();
        $item->contenthash = sha1('forum:' . $forumrec->id . ':' . sha1($document));
        $item->filename = $filename;
        $item->mimetype = 'text/html';
        $item->display = $forumtitle . ' (forum)';
        $item->sizelabel = display_size(strlen($document));
        $item->file = null;
        $item->payload = $document;
        // Slice-2 source identity. Per-forum granularity so a change
        // anywhere in any kept thread triggers re-upload + orphan
        // of the previous version of the same forum's doc.
        $item->sourceref = 'forum:' . $forumrec->id;
        return $item;
    }

    /**
     * Bounded, charset-safe filename slug. Mirrors the helper in
     * sync_materials; duplicated here so the forum collector stays
     * self-contained and can be unit-tested independently.
     *
     * @param string $name
     * @return string
     */
    private static function safe_slug(string $name): string {
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
