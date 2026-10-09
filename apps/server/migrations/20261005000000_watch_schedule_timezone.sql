-- Legacy watches retain their UTC cron expressions and semantics.
ALTER TABLE watches ADD COLUMN timezone TEXT;
