-- Per-exam scoring of multiple-answer questions.
--   all_or_nothing: full points only for the exact set of correct options.
--   partial:        correct picks earn a share, wrong picks subtract (never below 0).
-- New exams default to partial; exams that already exist keep all_or_nothing so the
-- grades already given do not change.
CREATE TYPE scoring_mode AS ENUM ('all_or_nothing', 'partial');

ALTER TABLE exams ADD COLUMN scoring scoring_mode NOT NULL DEFAULT 'all_or_nothing';
ALTER TABLE exams ALTER COLUMN scoring SET DEFAULT 'partial';

-- Partial credit makes earned points fractional.
ALTER TABLE attempts ALTER COLUMN earned_points TYPE double precision;
