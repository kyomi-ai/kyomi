-- KYO-249: correct the KYO-172/173 backfill's mis-classification of
-- historical `dashboard`, `knowledge`, and `chat_session` sync_log rows as
-- private.
--
-- 20260724000000_add_sync_log_visibility.sql backfilled every historical row
-- for these three types to is_workspace_visible = false unconditionally
-- (see its second and third UPDATEs). That is wrong in the *opposite*
-- direction from KYO-237's watch bug: visibility for these types is computed
-- per row at write time --
--   - dashboard/knowledge: dashboard_service::is_doc_publicly_visible
--     (dashboard_service.rs:1903) -- true when the doc belongs to a public
--     collection, via collection_dashboards -> collections.is_public. Write
--     sites: create_dashboard (:453), update_dashboard (:749),
--     delete_dashboard (:828).
--   - chat_session: the session's `shared` flag (chat_service.rs:922-923 and
--     other write sites), true when the session has been shared to the
--     workspace.
-- A dashboard sitting in a public collection (or a chat session shared to
-- the workspace) before 2026-07-24 has historical rows stuck at
-- is_workspace_visible = false forever, while the entity's *new* rows are
-- correctly written true. sync_log_service::get_entries_since filters on
-- `is_workspace_visible = TRUE OR owner_user_id = $3`
-- (sync_log_service.rs:223), so a non-owner whose delta cursor predates the
-- migration never receives those historical rows until a full re-bootstrap.
-- This errs toward undersync, the opposite of KYO-237 -- nothing is
-- over-exposed by the bug this migration fixes.
--
-- Do not edit 20260724000000 in place -- it has already applied and sqlx
-- checksums it. This migration corrects the affected rows instead.
--
-- Guard: unlike KYO-237 (which could key off `owner_user_id IS NULL` because
-- the buggy watch backfill left that column NULL while every write path
-- sets it), that signature does not exist here -- 20260724000000's backfill
-- extracted owner_user_id correctly for dashboard/knowledge/chat_session
-- (data->>'user_id' / data->'created_by'->>'user_id'), so a NULL-owner guard
-- cannot distinguish a backfilled row from a correctly-written one.
--
-- Instead this migration guards on `is_workspace_visible = false` and
-- recomputes it from CURRENT truth, using the exact predicate that already
-- governs live access for each entity type:
--   - dashboard/knowledge: the same collection_dashboards/collections join
--     dashboard_service::visibility_predicate (dashboard_service.rs:80) and
--     is_doc_publicly_visible (:1903) use.
--   - chat_session: a join to chat_sessions.shared -- the same column
--     get_session_info's is_shared_in_workspace check reads
--     (chat_service.rs:704-708) -- not `data->>'shared'` off the row's own
--     payload. The payload reflects the share state *at the time that row
--     was written*, which can go stale (a session can be shared, then
--     unshared, then shared again); the live predicate can only ever be the
--     session's *current* shared column. Recomputing from the payload could
--     mark a row workspace-visible whose current state is private -- the
--     exact leak this migration exists to avoid introducing on the other
--     type.
--
-- This is safe in a way that does not mirror KYO-237's risk:
--   1. It cannot leak. Every predicate above is the same one that already
--      gates live reads for that entity type. Anything this migration marks
--      workspace-visible was already independently readable by those users
--      through the normal query path -- there is no new exposure.
--   2. It never clobbers a correct row's data. The only column changed is
--      is_workspace_visible, and only false -> true; owner_user_id is never
--      touched. Contrast KYO-237, whose danger was overwriting
--      owner_user_id with NULL for an already-correct row.
--   3. The one case where this deliberately does not reconstruct exact
--      history: a doc/session that was private when a historical row was
--      written and has since become public/shared. This migration marks
--      that row visible too, because it recomputes from current truth
--      rather than write-time truth. For a cache-reconciliation mechanism
--      like sync_log, converging on current truth is the desired end state,
--      not a bug -- the alternative (leaving it false forever) is just a
--      different, permanent kind of staleness for a doc that is now
--      genuinely public.
--
-- Known residual gap, not fixed here: a Delete row for a dashboard or chat
-- session that has since been fully deleted cannot be recomputed at all --
-- collection_dashboards rows CASCADE-delete with their dashboard
-- (collection_dashboards_dashboard_id_fkey), and a deleted chat_sessions row
-- simply no longer exists to join against. The EXISTS below finds no match,
-- is_workspace_visible stays false, and a non-owner who had the entity
-- cached from before this fix never learns it was deleted -- it lingers in
-- their local cache indefinitely. This is pre-existing behaviour, symmetric
-- between dashboards and chat sessions, and predates this migration rather
-- than being introduced by it.
UPDATE sync_log SET is_workspace_visible = true
 WHERE entity_type IN ('dashboard', 'knowledge')
   AND is_workspace_visible = false
   AND EXISTS (
         SELECT 1 FROM collection_dashboards cd
         JOIN collections c ON cd.collection_id = c.id
         WHERE cd.dashboard_id = sync_log.entity_id
           AND c.is_public = TRUE
       );

UPDATE sync_log SET is_workspace_visible = true
 WHERE entity_type = 'chat_session'
   AND is_workspace_visible = false
   AND EXISTS (
         SELECT 1 FROM chat_sessions cs
         WHERE cs.session_id = sync_log.entity_id
           AND cs.shared = TRUE
       );
