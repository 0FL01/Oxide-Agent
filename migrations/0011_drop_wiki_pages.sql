-- Remove the retired LLM Wiki storage table while keeping migration 0005 in
-- the ledger for existing databases that have already applied it.
DROP TABLE IF EXISTS wiki_pages;
