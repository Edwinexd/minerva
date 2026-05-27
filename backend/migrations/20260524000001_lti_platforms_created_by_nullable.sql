-- Dynamic Registration installs an `lti_platforms` row from a public,
-- Shib-bypassed `/lti/dynamic-register` endpoint (the LMS popup hits it
-- with the platform's `registration_token` as the source of trust; tool-
-- side auth would just break the IMS dynreg flow). That endpoint has no
-- logged-in Minerva user to attribute the row to, so make `created_by`
-- nullable. NULL therefore means "registered via Dynamic Registration";
-- manual platform creates via the admin UI continue to set it.

ALTER TABLE lti_platforms ALTER COLUMN created_by DROP NOT NULL;
