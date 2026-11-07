-- SSO identity is the IdP's (issuer, subject), never the email claim.
--
-- Up to now an OIDC sign-in matched the verified email against every account.
-- Anyone who could set their own email at the IdP to a parent's address (and
-- get it reported as verified, which some IdP setups allow) got that parent's
-- session. The callback now matches only a bound (issuer, subject); a binding
-- is made by the first-run SSO signup or by "Link SSO" from inside a signed-in,
-- confirmed session. Existing accounts start unbound: they sign in another way
-- once and link it.
ALTER TABLE admins ADD COLUMN oidc_issuer text;
ALTER TABLE admins ADD COLUMN oidc_subject text;
CREATE UNIQUE INDEX idx_admins_oidc_identity ON admins (oidc_issuer, oidc_subject)
    WHERE oidc_subject IS NOT NULL;
