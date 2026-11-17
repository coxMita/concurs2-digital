-- Exam integrity signals recorded during an attempt (shown on the dashboard and in
-- the Excel export). Durations are measured by the server, not the browser.
ALTER TABLE attempts
    ADD COLUMN focus_losses        integer     NOT NULL DEFAULT 0,
    ADD COLUMN focus_away_ms       bigint      NOT NULL DEFAULT 0,
    -- Set while the student is outside the exam page; cleared when they return.
    ADD COLUMN away_since          timestamptz,
    ADD COLUMN copy_attempts       integer     NOT NULL DEFAULT 0,
    ADD COLUMN screenshot_attempts integer     NOT NULL DEFAULT 0;
