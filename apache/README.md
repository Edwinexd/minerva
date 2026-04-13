# Apache config (production)

These files are deployed to the prod host (`minerva.dsv.su.se`) by the
`Deploy to Production` workflow (see `.github/workflows/deploy.yml`).
The DSV-managed `shib-dsv.conf` is left alone.

## Files

| Repo                                          | Server path                                             |
| --------------------------------------------- | ------------------------------------------------------- |
| `apache/minerva-app.conf`                     | `/etc/apache2/sites-enabled/minerva-app.conf`           |
| `apache/minerva-ext-auth.lua`                 | `/etc/apache2/lua/minerva-ext-auth.lua`                 |
| `apache/install/minerva-apache-reload.path`   | `/etc/systemd/system/minerva-apache-reload.path`        |
| `apache/install/minerva-apache-reload.service`| `/etc/systemd/system/minerva-apache-reload.service`     |
| (manual)                                      | `/etc/apache2/secrets/minerva-hmac` (mirror of `MINERVA_HMAC_SECRET`) |

## How auto-deploy works

CI never `sudo`s. `apache/install/bootstrap.sh` (run once by a human admin)
chgrps the two managed files to the `apache-deploy` group and adds the
`ci` user to that group, so `ci` can `scp` updates straight onto them.
A systemd path unit watches both files and runs `apache2ctl configtest`
+ `systemctl reload apache2` on change. The service writes a timestamp
to `/var/log/minerva-apache-reload.stamp` so the deploy step can confirm
the reload happened.

If `apache2ctl configtest` fails, apache stays on its previous in-memory
config. The bad file remains on disk for inspection -- look at
`journalctl -u minerva-apache-reload`.

## First-time bootstrap (human admin, once)

```bash
# 1. SSH to the server as a user with sudo
ssh minerva

# 2. Clone the repo (or scp the apache/install/ folder onto the host)
git clone git@github.com:Edwinexd/minerva.git /tmp/minerva-bootstrap
sudo bash /tmp/minerva-bootstrap/apache/install/bootstrap.sh

# 3. Install the HMAC secret (matches MINERVA_HMAC_SECRET in k8s)
sudo KUBECONFIG=/etc/rancher/k3s/k3s.yaml \
    kubectl get secret -n minerva minerva-secrets \
    -o jsonpath='{.data.MINERVA_HMAC_SECRET}' | base64 -d \
    | sudo tee /etc/apache2/secrets/minerva-hmac >/dev/null
sudo chown root:www-data /etc/apache2/secrets/minerva-hmac
sudo chmod 0640 /etc/apache2/secrets/minerva-hmac

# 4. Trigger the first deploy (push or workflow_dispatch)
gh workflow run deploy.yml
```

After that, every push to `master` updates apache automatically.

## Local tests

`apache/test/run.sh` runs the pure-Lua HMAC/SHA-256 vectors (RFC 4231)
and the token-verify edge cases. Requires `lua5.4`.

```bash
brew install lua            # macOS, or `apt install lua5.4` on Linux
bash apache/test/run.sh
```

CI runs the same suite plus an `apache2ctl configtest` against the
vhost in a Debian runner with mod_lua + mod_shib installed.

## How the cookie path actually flows

1. `GET /api/external-auth/callback?token=...` → backend verifies token
   + `external_auth_invites` row → `302 /` with
   `Set-Cookie: minerva_ext=<token>; HttpOnly; Secure; SameSite=Lax`.
2. Subsequent `GET /<anything>`:
   - mod_rewrite spots the cookie, internally rewrites the URI to
     `/__ext_proxy__/<original>`.
   - `<LocationMatch ^/__ext_proxy__/>` runs `LuaHookAccessChecker`
     (no Shib here). Lua reads the cookie, validates HMAC + expiry,
     sets `eppn`, `displayName`, `X-Minerva-Ext-Jti` request headers.
   - `ProxyPassMatch` strips the prefix and forwards to the backend.
3. Backend `auth_middleware` reads `eppn` (same path as Shib users) and
   for `ext:`-prefixed eppns additionally looks up the JTI in the DB to
   enforce per-invite revocation. On any failure it returns 401 with a
   `Set-Cookie: minerva_ext=; Max-Age=0` header so the frontend's
   reload-on-401 doesn't loop.

## Why this shape

- mod_lua's stock build on Debian Apache 2.4 has no HTTP client, no
  subrequest API, no HMAC, no base64. SHA-256 is therefore implemented
  in pure Lua (see `minerva-ext-auth.lua`); the test suite covers it
  against RFC 4231 vectors.
- The `/__ext_proxy__/` URL prefix is internal-only; mod_rewrite is the
  only way to reach it. The prefix is a vehicle for attaching different
  auth directives to otherwise-identical paths -- using `<If>` inside
  `<Location />` was tried and broken because it short-circuits the
  carve-outs for `/api/health`, `/lti`, etc.
- `RequestHeader unset eppn early` strips client-supplied identity
  headers before any auth runs, so the bypass path can't be header-
  spoofed even though Shib is off there.
- mod_lua on this build doesn't expose `apache2.HTTP_*` constants; the
  script returns raw integer status codes (401, 500, etc.).
