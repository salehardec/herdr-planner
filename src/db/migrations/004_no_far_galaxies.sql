-- The distant galaxies (other machines of the board over ssh) are gone from the planner: forget what was kept for
-- them. The cache of their boards, the lag cache (only ever written) and the search documents of other machines.
DROP TABLE IF EXISTS galaxies;
DROP TABLE IF EXISTS gitlag;
DELETE FROM search_docs WHERE galaxy <> '';
DROP INDEX IF EXISTS search_docs_galaxy;
ALTER TABLE search_docs DROP COLUMN galaxy;
