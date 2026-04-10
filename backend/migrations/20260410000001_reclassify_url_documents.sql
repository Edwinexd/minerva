-- Reclassify URL documents from 'unsupported' back to 'pending'.
-- These are now handled by the external transcript pipeline instead of
-- being permanently marked unsupported by the worker.
UPDATE documents SET status = 'pending', error_msg = NULL
WHERE mime_type = 'text/x-url' AND status = 'unsupported';
