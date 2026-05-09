-- Add locality-aware load balancing support.
-- This migration adds the `zone` column to the `targets` table.

ALTER TABLE targets ADD COLUMN zone TEXT;
