Dev-only credentials for the local libSQL server (`scripts/dev-db.sh up libsql`, container `dbear-libsql`).

- `jwt_public_key`: Ed25519 public key (base64url) passed to sqld as `SQLD_AUTH_JWT_KEY`.
- `dev_token`: a non-expiring JWT signed with the matching private key (discarded). Use it as the auth token.

They only protect a throwaway server on 127.0.0.1; never use them anywhere else.
