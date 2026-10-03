-- Resumable gap repair: blocks are repaired newest-first, and everything in
-- [progress_slot, to_slot] is already done. NULL = not started.
ALTER TABLE gaps ADD COLUMN progress_slot BIGINT;
