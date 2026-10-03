# Quality checkpoint: S1–S5 (2026-10-03)

Bug tracking for the quality checkpoint taken after S5 of the redesign ("the show", Epic 1).
The decisions themselves live in [PLAN.md](PLAN.md); this file tracks what was found, where,
and what happened to it.

There were two rounds:

1. **Review.** Five reviewers read the S1–S5 code (about 10.5k lines since #29). Claude checked
   every serious finding in the code. All fixes shipped in #42–#45.
2. **QA scouts.** Five scouts hunted for new bugs: API, security, live show, web and media.
   Each finding comes with a failing test or a reproducible input. The fixes shipped in
   four PRs, #46–#49.

**Status key**
- ✅ fixed, merged and deployed
- 🔧 fix in progress (branch named)
- ⏸ open, not scheduled
- ❌ won't fix (reason given)
- ❓ needs a decision

**Severity:** H high · M medium · L low

---

## Round 1: review (all ✅ unless noted)

Deployed in order:

| PR | Commit |
|---|---|
| #42 | `049e2a7` |
| #43 | `d7ef532` |
| #44 | `9df4c73` |
| #45 | `f6046a5` |

### Shows and hold (#43)

| ID | Sev | Where | Bug | Status |
|---|---|---|---|---|
| R-01 | H | core/shows.rs `tonight`, `create` | Clips uploaded in the lobby, or still processing when a show opened, dropped out of every future lineup. | ✅ #43: tonight = ready, never played in an ended show, since the anchor; lobby uploads join at `start` |
| R-02 | M | core/clips.rs `hold` | A hold could be renewed forever (`now() + 7 d` on every call), even after release or play. | ✅ #43: `created_at + 7 d`, refused once played; migration 0012 |
| R-03 | M | api/live.rs `Load` | Loading a clip in the lobby released its hold without counting it as played. | ✅ #44: load/play only while live; release on play |
| R-04 | L | core/shows.rs `vote` | A vote could land after `end` had tallied. | ✅ #43: conditional insert plus a row lock |
| R-05 | L | core/shows.rs `set_lineup` | Positions collided when clips were played out of order. | ✅ #43: renumbered by `played_at`; duplicates rejected |
| R-06 | L | core/shows.rs `react`, `fail_contenders` | Reactions and 🍌 were accepted on clips that hadn't played. | ✅ #43 |
| R-07 | L | core/shows.rs `vote`, tally | Trashed clips could be voted for and win. | ✅ #43 |
| R-08 | L | api/routes/members.rs | Profile counts included other people's held clips. | ✅ #43 |
| R-09 | L | api/routes/clips.rs `teaser_view` | Admins got Edit/Delete on teasers that then 404'd. | ✅ #43: `canEdit: false` |
| R-10 | L | api/routes/shows.rs `end` | `POST /end` required a JSON body. | ✅ #43: optional body |
| R-11 | L | core/shows.rs `end` | A double tie needed two round trips. | ✅ #43: one 409 lists both |
| R-12 | L | api/routes/shows.rs `list_shows` | N+1 queries per past show. | ⏸ needs batched queries; fix with S7 (past shows) |
| R-13 | M | api/routes/clips.rs `visible_clip` | The next clip couldn't be preloaded when held, which is most of a show. | ✅ #43: participants of a **live** show can fetch lineup clips |

### Live hub (#44)

| ID | Sev | Where | Bug | Status |
|---|---|---|---|---|
| R-14 | H | api/live.rs room creation | No takeover after a restart if the host never came back, and no abandon either. | ✅ #44 |
| R-15 | M | api/live.rs `serve` | Ghost connections: a failed welcome skipped cleanup, leaving the user "online" forever. | ✅ #44: drop guard |
| R-16 | M | api/live.rs `TakeOver` | Takeover race: two winners, and the database and memory disagreed on the host. | ✅ #44: compare-and-set `set_host` |
| R-17 | M | api/live.rs `run` | No liveness check, so a sleeping host stayed online for about 15 min. | ✅ #44: 4008 after 75 s of silence |
| R-18 | M | api/live.rs `connect` | No message size limit (64 MiB, even before auth). | ✅ #44: 16 KiB |
| R-19 | M | api/live.rs | Connections outlived their show. | ✅ #44: `showOver` + 4004 |
| R-20 | M | api/live.rs | Auth was only checked at hello: expiry, disable and the shows gate were ignored. | ✅ #44: closes at `exp`, re-checks each tick |
| R-21 | L | api/live.rs | No rate limit on react/ready/replay. | ✅ #44: 10/s, burst 20 |
| R-22 | L | api/live.rs `commit` | Two host tabs could save live_state out of order, and `atServerMs` was stamped before the DB writes. | ✅ #44: save newer only; stamp after |
| R-23 | L | api/live.rs | `mark_played` failures were logged at debug level only. | ✅ #44: warn, plus an error to the host |
| R-24 | L | api/live.rs | No takeover when the clip's duration was unknown. | ✅ #44 |
| R-25 | L | api/live.rs `Play` | Play while already playing rescheduled, freezing everyone. | ✅ #44: no-op |

### Web (#45)

| ID | Sev | Where | Bug | Status |
|---|---|---|---|---|
| R-26 | H | routes/Upload.tsx | After Cancel the page wouldn't take a new file. | ✅ #45 |
| R-27 | M | routes/Upload.tsx, lib/upload.ts | Cancel couldn't stop requests that hadn't started, leaving orphan clips. | ✅ #45: signal everywhere; cancelled clip deleted |
| R-28 | M | routes/Upload.tsx | The hold switch only applied on "Save details"; the default was wrong when `/me` loaded late. | ✅ #45 |
| R-29 | M | show/clock.ts | A stale clock sample after sleep could put the clock about 10 min off. | ✅ #45: samples reset and expire; sleep detection |
| R-30 | M | routes/ShowLive.tsx | The old clip replayed from 0 when the next clip loaded. | ✅ #45: engine per clip; stable `src` |
| R-31 | L | show/live.ts | Reconnected forever after a terminal refusal. | ✅ #45: close codes |
| R-32 | L | show/live.ts | Reported the best RTT instead of the latest. | ✅ #45 |
| R-33 | L | show/sync.ts | Any `play()` rejection counted as "blocked"; an ended clip could restart from 0; it seeked while buffering. | ✅ #45 |
| R-34 | L | show/useShowLive.ts | State wasn't reset on show change; reactions arriving together were lost. | ✅ #45 |
| R-35 | M | routes/Archive.tsx | The search box wrote `q` back, so it couldn't be cleared by navigating. | ✅ #45 |
| R-36 | M | routes/Archive.tsx | The restyle lost sort, the map filter and the uploader chip; "All" didn't clear everything. | ✅ #45 |
| R-37 | M | routes/ClipPage.tsx | Failed clips lost Download original; admins couldn't delete them; Delete showed on trashed clips. | ✅ #45 |
| R-38 | L | routes/ClipPage.tsx | Processing marked "Uploaded" done while still uploading; the HeldNotice error was silent. | ✅ #45 |
| R-39 | L | useMe.ts, hooks | Profile and member caches went stale after edits. | ✅ #45 |
| R-40 | L | UserPage, SharePanel, Modal | Tab lost on close; stale confirmation on re-share; `onClose` ran twice. | ✅ #45 |
| R-41 | L | e2e/pages.ts | The not-found page wasn't checked on phones; empty archive had no call to action; Panel and Toast were unused. | ✅ #45 |

### Worker (#42)

| ID | Sev | Where | Bug | Status |
|---|---|---|---|---|
| R-42 | H | migrations/0011, core/jobs.rs `claim` | The keyframes backfill starved new uploads. | ✅ #42: job priority (0013) |
| R-43 | M | worker/transcode.rs rekey | A re-encode overwrote the playback blob someone might be watching. | ✅ #42: new names; delayed `delete_blobs` job |
| R-44 | L | worker/transcode.rs | A probe failure was treated as "re-encode". | ✅ #42 |
| R-45 | L | worker/transcode.rs | The keyframe gap was measured from t=0, not from the first packet. | ✅ #42 |

### Round 1 leftovers

| ID | Sev | Where | Item | Status |
|---|---|---|---|---|
| R-46 | L | api/routes/share.rs `video` | A share-link viewer mid-play during a re-encode gets the new file's bytes. Re-encodes only came from the one-off backfill, which is done. | ⏸ known note |
| R-47 | L | worker teaser | Clips transcoded before S3 have no teaser. Mitigated: R-02 means only clips under 7 days old can be held. | ⏸ |
| R-48 | L | PLAN.md vs Player.tsx | The plan says "your kills" are red; the code draws them amber. | ❓ which one is right |
| R-49 | L | vite.config.ts | The dev proxy doesn't set `ws: true`, so the show page may not connect under `pnpm dev`. | ✅ fixed in this PR: confirmed the upgrade wasn't proxied; `ws: true` |
| R-50 | L | security.rs `img-src` | Gravatar default avatars may redirect to a host outside the CSP. | ⏸ unverified |

---

## Round 2: QA scouts

The four fixer branches are `fix-worker-qa`, `fix-api-qa`, `fix-live-qa` and `fix-web-qa`.

### Media and worker → 🔧 `fix-worker-qa` (✅ #48 deployed `6f9a65b`: M-01–M-12 all fixed; M-13 won't fix)

| ID | Sev | Where | Bug | Repro |
|---|---|---|---|---|
| M-01 | H | worker/analyse.rs:145 | ffmpeg's stderr is never drained while frames are read. Past 64 KB the whole worker hangs, and `/healthz` stays green. | Heavily corrupted 60 s 1080p60 clip |
| M-02 | H | core/jobs.rs:129 `requeue_stale` | A job that crashes or hangs its worker is requeued forever and never reaches MAX_ATTEMPTS. | Test `qa_crash_loop_never_gives_up` |
| M-03 | M | worker/transcode.rs:584 | Odd heights (for example 1281×721) always fail as `server`. | VP9 MKV, FFV1, H.264 4:4:4 |
| M-04 | M | worker/transcode.rs:277 | The poster seeks past the last video frame (video shorter than audio, a 1-frame clip, a JPEG renamed .mp4). | `testsrc2` 2 s + `sine` 10 s |
| M-05 | M | worker/transcode.rs:723 | The first video stream is used, even when it's cover art or a thumbnail. | Cover-art-first MKV; a small stream first; audio plus a cover |
| M-06 | M | worker/transcode.rs:52 | A clip deleted then restored while processing stays stuck. | Test `qa_delete_then_restore_while_processing` |
| M-07 | M | worker/transcode.rs:204 | The too-long check is skipped when the container has no duration. | A 30-minute raw .h264 goes Ready |
| M-08 | M | worker/transcode.rs:433 | Keyframes are checked only in the first 30 s; one remux kept a 20 s gap. | `late_gop.mp4` |
| M-09 | L | worker/transcode.rs:337 | A rotated H.264 remux records landscape dimensions. | `-display_rotation 90` |
| M-10 | L | core/jobs.rs:95 | A late `fail()` from a stale run re-queues a finished job. | Test `qa_stale_duplicate_marks_ready_clip_failed` |
| M-11 | L | worker/transcode.rs | Picture fidelity: anamorphic SAR is lost; interlaced sources aren't deinterlaced. | 1440×1080 SAR 4:3 |
| M-12 | L | worker/main.rs:177 | Temp directories from a crash are never cleaned up. | Read in code |
| M-13 | L | worker/transcode.rs | HDR10 comes out 8-bit, still tagged BT.2020, with no tone-mapping. | ❌ known limitation: needs filters production's ffmpeg may lack, and HDR CS2 recordings are rare |

### API → 🔧 `fix-api-qa` (✅ #49 deployed `390c101`: A-01–A-14, L-05, L-06 fixed; A-06's worker-side ETag check is F-01)

| ID | Sev | Where | Bug | Repro |
|---|---|---|---|---|
| A-01 | M | api/routes/clips.rs:760, core/social.rs | Editing a clip that tags a disabled friend always 400s, and the failed edit is partly saved. | `qa_editing_a_clip_that_tags_a_disabled_friend`, `qa_a_rejected_patch_changes_nothing` |
| A-02 | M | core/social.rs:300 | `reaction_count` drifts under concurrent reactions (12 PUTs stored 10). | `qa_concurrent_reactions_keep_the_count` |
| A-03 | M | api/routes/share.rs:116 | A malformed `Range` on public share media returns 500, without signing in. | `qa_share_ranges` |
| A-04 | M | core/clips.rs:385 | A tampered page cursor returns 500. | `qa_pages_are_stable_when_timestamps_tie` |
| A-05 | M | core/clips.rs restore/purge | A restore racing the trash cleanup answers 200, then the clip is purged. | `qa_restore_racing_the_purge` |
| A-06 | M | api/routes/clips.rs:417 | The upload link stays writable for 2 h after `/complete`, so the 2 GB size check can be bypassed. | `qa_upload_complete_edges` |
| A-07 | L | core/social.rs:141 | Tag autocomplete reveals the names of held, trashed and hidden clips' tags (also security S-02). | `qa_tag_autocomplete_hides_held_and_hidden_clips` |
| A-08 | L | api/routes/clips.rs:645 | A tag filter that normalises to nothing (`?tag=!!!`) matches every clip. | `qa_a_junk_tag_filter_doesnt_match_everything` |
| A-09 | L | core/shows.rs:438 | Clips added to a show at the same time get the same position. | `qa_show_concurrency` |
| A-10 | L | core/shows.rs:608 | A show reaction on a trashed clip turns on its clip-page reaction. | `qa_show_reaction_on_a_trashed_clip` |
| A-11 | L | api/routes/share.rs:75 | A trashed clip's share link can't be revoked, and comes back on restore. | `qa_unshare_a_trashed_clip` |
| A-12 | L | all handlers | Body, path and query rejections return plain text instead of `ErrorBody`. | `qa_extractor_rejections_are_error_bodies` |
| A-13 | L | core/clips.rs:252 | Zero-width or RTL-override-only titles and newlines are accepted; the description is trimmed after the length check on create; a confusing message for a bigger-than-declared file. | `qa_clip_routes_matrix` |
| A-14 | L | api routes, openapi.rs | Small API inconsistencies, each with a recommended default: PATCH on a trashed clip; show bodies accept unknown fields; `map: null` doesn't clear the map; error statuses missing from OpenAPI; profile `clipCount` ≠ the `?uploader=` list. | API scout |
| L-05 | L | core/shows.rs:375 | A lineup edit racing a play marks the played clip as dropped (found by the live scout). | `qa_bug_a_lineup_edit_racing_play_drops_a_played_clip` |

### Security → 🔧 `fix-api-qa` (✅ #49 deployed `390c101`: S-01–S-03 fixed)

No sign-in or permission bypass was found.

| ID | Sev | Where | Bug | Repro / status |
|---|---|---|---|---|
| S-01 | M | core/shows.rs `add_clip`, api/routes/clips.rs `show_clip` | A participant can pull any held clip into a live show and watch it early. | ✅ #49 `qa_participant_cannot_pull_unlisted_held_clip_into_live_show` |
| S-02 | L | core/social.rs `search_tags` | Tag names of held clips leak (same as A-07). | ✅ #49 |
| S-03 | L | core/storage.rs | Signed SAS URLs end up in error text, logs and `jobs.last_error`. | ✅ #49 `qa_storage_errors_do_not_contain_sas_signatures` |
| S-04 | L | .github/workflows/ci.yml | `cargo deny` and `pnpm audit` only run when code changes; there's no scheduled run. | ✅ fixed in this PR: `audit.yml`, Mondays 06:00 UTC and by hand |
| S-05 | — | api/ratelimit.rs | Could the rate limit be dodged by faking `X-Forwarded-For`? | ✅ verified on production 2026-10-03: 80 requests with spoofed IPs, 75 → 404 and 5 → 429, so the real IP is used |
| S-06 | L | api/security.rs | Any `/s/<anything>` path may be framed, not only valid tokens. | ❌ negligible: it only renders the share or expired screen |
| S-07 | L | api/live.rs | A member can open many sockets; pings aren't rate-limited. | ⏸ low risk among friends |

### Live hub → 🔧 `fix-live-qa` (✅ #46 deployed `5302f7a`: L-01–L-04, L-07–L-09 fixed; L-05/L-06 ✅ in #49, deployed `390c101`)

| ID | Sev | Where | Bug | Repro |
|---|---|---|---|---|
| L-01 | M | api/live.rs:848 | Play broadcasts a clip that was dropped or purged after Load. | `qa_bug_play_after_the_loaded_clip_is_dropped` / `_purged` |
| L-02 | M | api/live.rs:824 | A trashed clip loads and plays (with no duration, takeover opens mid-clip). | `qa_bug_a_trashed_clip_loads_and_plays` |
| L-03 | M | api/live.rs:836 | Someone who joins during a slow commit gets a different state under the same seq and starts about 1 s ahead. | `qa_bug_a_joiner_mid_commit_gets_a_different_state_under_the_same_seq` |
| L-04 | L | api/live.rs:843 | `load.startAt` has no upper bound; a day ahead blocks takeover. | `qa_bug_load_start_at_has_no_upper_bound` |
| L-06 | L | core/shows.rs:802 (→ `fix-api-qa`) | An abandoned show brings a played clip back without its hold. | `qa_bug_abandon_brings_a_clip_back_without_its_hold` |
| L-07 | L | api/live.rs:65 | `ABANDON_AFTER` isn't part of `Timing`, so the hub's abandon path is untested. | Read in code |
| L-08 | L | api/live.rs | The comment on recovery after lag is wrong (the backlog is still delivered). | Read in code |
| L-09 | L | api/live.rs `tick` | A lobby with no sockets (REST-only `/tonight`) would be abandoned after 15 min. Risk for S6. | Read in code |

### Web → 🔧 `fix-web-qa` (✅ #47 deployed `cb2f28a`: W-01–W-16 all fixed)

Screenshots are in the session scratchpad `qa-web/`.

| ID | Sev | Where | Bug | Repro |
|---|---|---|---|---|
| W-01 | H | clips/killfeed/KillfeedPanel.tsx:187, Player.tsx:430/531 | Duplicate React keys: the kill list grows ghost rows with each Yours/All toggle. | Two kills in the same second with the same gun |
| W-02 | H | routes/Layout.tsx:59, useMe.ts:17 | A failed `/api/me` (one 500, or an expired sign-in) locks you out of the whole app: no Sign out, no retry. | `qa-desk-failures` "me 401", `qa-desk-flow` "me 500 once" |
| W-03 | M | clips/Player.tsx:179 | Player hotkeys (M, T, K, digits) fire behind open dialogs. | `qa-desk-keyboard` |
| W-04 | M | clips/hooks.ts:38/51, ClipPage.tsx:373 | A clip page opened while uploading never refreshes; an abandoned upload's page has no buttons. | `qa-desk-flow`, `qa-desk-clip` |
| W-05 | M | routes/Upload.tsx:253 | Leaving Upload by an in-app link silently loses the typed details. | `qa-desk-flow` "upload: leaving mid-upload" |
| W-06 | M | clips/Reactions.tsx:17 | Reactions lose keyboard focus and fail silently. | `qa-desk-keyboard` |
| W-07 | M | ClipPage.tsx:150, UserPage.tsx:271, ui/Button.tsx:12, ClipCard.tsx:47 | Accessibility: two links have no name; white on `#e5484d` is 3.91:1. | axe-core 4.11.3 |
| W-08 | L | ClipPage.tsx:444 | Focus is lost after the Delete dialog closes. | Keyboard |
| W-09 | M | ClipPage.tsx:84/189 | The clip page ignores `teaser`, so a participant can play a held clip early and sees the owner's "Post now". | `clip-teaser-other.png` |
| W-10 | M | clips/ClipGrid.tsx:44 | Archive pages that overlap aren't de-duplicated (possible under Top). | Overlapping mock |
| W-11 | L | Upload.tsx:40, lib/format.ts:3 | A 0-byte file is accepted ("1 KB"). | `upload-0-byte.png` |
| W-12 | L | ClipCard.tsx:63, h1s | RTL titles are cut at the wrong end (no `dir="auto"`). | `data-rtl-zoom.png` |
| W-13 | L | Archive.tsx:150, SharePanel.tsx:28 | No focus indicator on the search box and share-link field; focus rings are the browser's blue. | `search-focused.png` |
| W-14 | L | routes/ShowLive.tsx, show/live.ts:129 | Show page messaging: REST error hidden; bad JSON throws; empty error toast; a terminal close announced 2–3 times; a 4008 reconnect flashes red. | `show-odd-messages.png` |
| W-15 | L | various | Small issues: the "…" menu has no arrow keys; a broken avatar shows a broken image; a ready clip with no poster says "Processing…"; no skip link; every page titled "clipos". | QA scout |
| W-16 | L | various | Product defaults (recommended): Retry on error states; tell the user when several files are dropped; Back works across filter changes. | QA scout |

---

### Follow-ups found while merging (integration QA on #48 × #49)

| ID | Sev | Where | Item | Status |
|---|---|---|---|---|
| F-01 | M | worker/transcode.rs:393 `local_or_remote` | The worker never checks `clips.original_etag` (stored by #49), so an original overwritten after `/complete` through the still-valid SAS is transcoded. NULL for older clips is fine. | ✅ fixed in this PR: `If-Match` on download; ETag and size compared before and after a read over HTTP; size only when the ETag is NULL; a changed original fails for good as `unreadable` |
| F-02 | L | core/clips.rs:643 `restore` | A restore while a transcode is running queues a second job. That's harmless with one worker instance, but two workers (scale-out, or containers overlapping in a deploy) could both write the same blob names. | ⏸ by design for one instance; revisit on scale-out |
| F-03 | L | web/e2e/fixtures.ts:170, web/tsconfig.json | The e2e fixtures aren't typechecked against the API schema (`tsc` only covers `src`); the show fixture lacks `voters`. | ✅ fixed in this PR: `tsconfig.e2e.json` in `pnpm typecheck` |
| F-04 | L | api/live.rs `tick` | A show started from an empty lobby began its abandon clock at the lobby's last tick, not at the start. Found by #49's CI. | ✅ fixed in #49 (`6cbdbe0`), deployed `390c101` |
| F-05 | L | crates/api/tests/http.rs | Other abandon timing tests use about 100 ms margins against 300 ms. | ✅ fixed in this PR: ticks until abandoned (5 s at most); other checks at 600 ms |

### Found by the coverage push (#54, #55, #59)

| ID | Sev | Where | Item | Status |
|---|---|---|---|---|
| C-01 | M | worker/analyse.rs `sample_corner` | The killfeed corner was cropped before the RGB conversion; on 4:2:0 video an odd corner width (1600×900 → 675) was rounded down, so every frame was read skewed and the last one lost. | ✅ fixed in #59: crop after `format=rgb24`. Clips read before that: ✅ fixed in this PR with an admin "Re-analyse kill feed" (`POST /api/admin/clips/{id}/analyse`, in the clip page's … menu), not a migration; the analysis shows as pending until the new result replaces the old one |
| C-02 | M | worker/transcode.rs `make_poster` | A poster seek past the last frame made ffmpeg 7.1/8 exit with an error that was treated as retryable, so the first-frame fallback never ran and the clip failed. | ✅ fixed in #59 |
| C-03 | M | web/e2e (coverage merge) | Unit-test hits were lost in the merge (browser source maps applied to them as well), and a second page load erased that page's coverage. | ✅ fixed in #55 (`remap-coverage.mjs`, saves before each navigation) |
| C-04 | L | core/auth.rs `JwtVerifier` | `nbf` isn't checked (jsonwebtoken's default). Auth0 doesn't set it. | ✅ fixed in this PR: checked when present, with the 30 s leeway; still optional |
| C-05 | L | core/shows.rs `react` | The show reaction and the clip-page reaction are saved separately, not atomically. | ⏸ |
| C-06 | L | core/users.rs `sign_in` | The same email with a different Auth0 id fails with a misleading "could not allocate a handle". Unlikely while sign-in is Google-only. | ⏸ |
| C-07 | L | worker/analyse.rs | An original that ffmpeg can't decode at all is retried rather than failed for good. | ✅ fixed in this PR: kept as it was (retried, failed for good after `MAX_ATTEMPTS` = 3; the clip stays ready, without an analysis), now pinned by a worker test |
| C-08 | L | local test runs | sqlx names test databases after the test, so concurrent local runs on one Postgres server can drop each other's databases. | ⏸ use separate servers locally |

## Decisions taken during the checkpoint

| Date | Question | Decision |
|---|---|---|
| 2026-10-03 | Clips that never play keep coming back to every show. | A clip dropped in two ended shows stops coming back (PLAN decision 28). |
| 2026-10-03 | A disabled or demoted host leaves the show stuck over REST. | Leave as is: takeover in the live show is the only way out. |
| 2026-10-03 | Should an abandoned show give played clips their hold back? | Yes, restore the hold (still capped at 7 days from upload). |
| 2026-10-03 | Should others' failed or processing clips be reachable by link? | No: 404 for everyone but the uploader and admins until ready. |
| 2026-10-03 | Should live vote counts be visible during the finale? | No: only who has voted is shared; counts appear at the reveal. |
| 2026-10-03 | How strict is the coverage gate? | 95 % of lines for Rust and web (`rust-coverage`, `web-coverage`). |
| 2026-10-03 | How do CI and CD fit together? | PR gates only: tests with coverage and the checks, every job on its own in parallel, on each push to the branch. A PR is merged only when its gates are green on its last commit and the branch holds the latest main (update and re-run otherwise). CD runs automatically on main after the merge (deploy for app changes, `tofu apply` for the stacks changed), behind `gate.yml`, which refuses a stale branch, a direct push or failed CI. An automatic apply refuses plans that destroy or replace resources; those and any override go through Run workflow. No branch protection (private repo on GitHub Free), so the merge rule and the CD gate enforce it. |
| 2026-10-03 | Should tokens' `nbf` be checked (C-04)? | Yes: checked when a token has one, with the 30 s leeway; still optional, as Auth0 doesn't send it. |
| 2026-10-03 | An original ffmpeg can't decode: retry its analysis or fail it at once (C-07)? | Keep retrying; it fails for good after 3 attempts, and the clip stays ready without an analysis. |
| 2026-10-03 | How are clips read with the killfeed-corner bug fixed (C-01)? | An admin-only "Re-analyse kill feed" on the clip page, for these and any future case; no migration. |

## Next

1. ✅ Review, merge and deploy the four QA fix PRs (merges need the user's OK): #46–#49,
   the last deployed as `390c101`.
2. ✅ Coverage to at least 90% lines, Rust and web, with a CI gate. From the starting points:
   - Rust 78.2% → 99.0% (`cargo llvm-cov`, #59);
   - web 76.4% → 99.5% (Vitest plus Playwright merged, #54 and #55).
   The gate is 95 %, and coverage runs first (decisions above).
3. Then S6: the show screens.
