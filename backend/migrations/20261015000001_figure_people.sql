-- Crops of people (webcam and video-call tiles, participant name tiles,
-- portraits) are withheld from description and search. A CLIP screen
-- scores every crop before anything else sees it; NULL means not yet
-- screened, and unscreened crops are neither described nor indexed.
-- shows_person is also set when the description model reports a person
-- the screen missed; those crops then act as near-duplicate exemplars.
ALTER TABLE document_figures
    ADD COLUMN people_score REAL,
    ADD COLUMN shows_person BOOLEAN;

CREATE INDEX idx_document_figures_unscreened
    ON document_figures(created_at) WHERE shows_person IS NULL;
