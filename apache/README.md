# Apache config (production)

These files live on the prod server (`minerva.dsv.su.se`), not in any container.
They are **manually deployed** -- Apache there is hand-maintained, not
DSV-managed (the DSV-managed `shib-dsv.conf` is left alone).

## Files

- `minerva-app.conf` → `/etc/apache2/sites-enabled/minerva-app.conf`
- `minerva-ext-auth.lua` → `/etc/apache2/lua/minerva-ext-auth.lua`

## First-time install / external-auth rollout

```bash
ssh minerva
sudo a2enmod lua headers rewrite                 # rewrite/headers already on; lua is new
sudo install -d -m 0755 /etc/apache2/lua
# Copy from your checkout (rsync, scp, or paste):
sudo cp /path/to/repo/apache/minerva-ext-auth.lua /etc/apache2/lua/
sudo cp /path/to/repo/apache/minerva-app.conf /etc/apache2/sites-enabled/
sudo apache2ctl configtest
sudo systemctl reload apache2
```

## Verifying the cookie path

End-to-end smoke test: mint an invite via the admin UI, click the link in a
private browser window. The expected sequence:

1. `GET /api/external-auth/callback?token=...` → `302 /` with `Set-Cookie: minerva_ext=...`
2. `GET /` → mod_rewrite matches the cookie, rewrites to `/__ext_proxy__/`,
   Lua hook subrequests `/api/external-auth/verify`, backend returns 200 +
   `X-Minerva-Eppn` header, Lua sets `eppn` request header, request proxies
   to backend at port 30090, backend's `auth_middleware` sees the eppn and
   serves the SPA.

Failure modes worth checking:

- Bad cookie (`minerva_ext=garbage`) → Apache returns 401 (Lua hook gets
  non-200 from verify subrequest).
- No cookie → mod_rewrite doesn't fire, request hits Shib path as normal.
- Revoked invite → backend's verify returns 401, Apache returns 401.

## Why this shape

- mod_lua is in Apache (one less moving part than a separate auth daemon)
  but Lua doesn't ship HMAC/base64 -- so all crypto stays in the Rust app
  via the `/api/external-auth/verify` subrequest.
- The `/__ext_proxy__/` URL prefix is internal-only; mod_rewrite is the
  *only* way to reach it (no external request can; LocationMatch checks
  the Lua hook, not the prefix). The prefix is purely a vehicle for
  attaching different auth directives to otherwise-identical paths.
- `RequestHeader unset eppn early` runs before any auth, so even on the
  bypass path a client cannot inject their own `eppn`. mod_lua puts the
  header back from the verified subrequest response.
