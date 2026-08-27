-- Anti-phishing for client-first login: a short code shown identically in the
-- browser and on the approving device, so a human only taps approve when the
-- numbers match — defeating push-approval (MFA-fatigue) phishing where an
-- attacker triggers the prompt for a name they know.
ALTER TABLE login_requests ADD COLUMN match_code text NOT NULL DEFAULT '----';
