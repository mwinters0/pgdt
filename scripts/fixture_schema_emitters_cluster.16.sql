-- The `emitters` cluster script's v16 sidecar: DDL 13 to 15 refuse, run
-- after fixture_schema_emitters_cluster.sql at 16 and above. A membership
-- granted without `INHERIT` or `SET`, which `dumpRoleMembership` writes from
-- 16 as `WITH INHERIT FALSE, SET FALSE`.
GRANT emitters_grantor TO emitters_other WITH INHERIT FALSE, SET FALSE;
