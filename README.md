Nginx Cache Purge
====================

[![CI](https://github.com/magiclen/nginx-cache-purge/actions/workflows/ci.yml/badge.svg)](https://github.com/magiclen/nginx-cache-purge/actions/workflows/ci.yml)

An alternative way to do `proxy_cache_purge` or `fastcgi_cache_purge` for Nginx.

## Usage

### Installation / Uninstallation

From [crates.io](https://crates.io/crates/nginx-cache-purge),

```bash
cargo install nginx-cache-purge

# cargo uninstall nginx-cache-purge
```

From GitHub (Linux x86_64),

```bash
curl -fL "$(curl -fsS https://api.github.com/repos/magiclen/nginx-cache-purge/releases/latest | sed -r -n 's/.*"browser_download_url": *"(.*\/nginx-cache-purge_'$(uname -m)')".*/\1/p')" -O && sudo mv nginx-cache-purge_$(uname -m) /usr/local/bin/nginx-cache-purge && sudo chmod +x /usr/local/bin/nginx-cache-purge

# sudo rm /usr/local/bin/nginx-cache-purge
```

### CLI Help

```
EXAMPLES:
nginx-cache-purge p /path/to/cache 1:2 http/blog/             # Purge the cache with the key "http/blog/" in the "cache zone" whose "path" is /path/to/cache, "levels" is 1:2
nginx-cache-purge p /path/to/cache 1:1:1 'http/blog*'         # Purge the caches with the key which has "http/blog" as its prefix in the "cache zone" whose "path" is /path/to/cache, "levels" is 1:1:1
nginx-cache-purge p /path/to/cache 2:1 '*/help*'              # Purge the caches with the key which contains the substring "/help" in the "cache zone" whose "path" is /path/to/cache, "levels" is 2:1
nginx-cache-purge p /path/to/cache '' 'http/blog*'            # Purge the caches in the "cache zone" whose "path" is /path/to/cache and which has no "levels"
nginx-cache-purge p /path/to/cache 1 '*'                      # Purge all caches in the "cache zone" whose "path" is /path/to/cache, "levels" is 1
nginx-cache-purge p /path/to/cache 2 '*' -e 'http/static/*'   # Purge all caches except for those whose key starts with "http/static/" in the "cache zone" whose "path" is /path/to/cache, "levels" is 2
nginx-cache-purge p /path/to/cache 1:2 '*' --dry-run          # List the caches that would be purged without removing anything
nginx-cache-purge s                                           # Start a server which listens on "/tmp/nginx-cache-purge.sock" to handle purge requests
nginx-cache-purge s /run/nginx-cache-purge.sock               # Start a server which listens on "/run/nginx-cache-purge.sock" to handle purge requests
nginx-cache-purge s --zone my_cache /path/to/cache 1:2        # Start a server which only accepts purge requests for the "my_cache" zone

Usage: nginx-cache-purge <COMMAND>

Commands:
  purge  Purge the cache immediately [alias: p]
  start  Start a server to handle purge requests [alias: s]
  help   Print this message or the help of the given subcommand(s)

Options:
  -h, --help     Print help
  -V, --version  Print version
```

If the `purge` command successfully removes any cache, it returns the exit status **0**. If no cache needs to be removed, it returns the exit status **44**.

### Key Patterns

A `*` in a key matches any sequence of characters. A key without a trailing `*` has to match the whole cache key.

| Pattern | Matches |
| ------- | ------- |
| `http/blog/` | exactly the key `http/blog/` |
| `http/blog*` | every key which starts with `http/blog` |
| `*/help` | every key which ends with `/help` |
| `*/help*` | every key which contains `/help` |
| `*` | every key |

### Levels

The `levels` argument has to be the same as the one set by `proxy_cache_path` or `fastcgi_cache_path`. Since `levels` is optional for Nginx, it may be empty here as well, which means every cache file sits directly in the cache directory.

### Nginx + Nginx Cache Purge

#### Start the Service of Nginx Cache Purge (systemd for example)

Assume we have already put the executable file `nginx-cache-purge` in `/usr/local/bin/`.

**/etc/systemd/system/nginx-cache-purge.service**

```
[Unit]
Description=Nginx Cache Purge
After=network.target
 
[Service]
# same as the user/group of the nginx process
User=www-data
Group=www-data

ExecStart=/usr/local/bin/nginx-cache-purge start --zone my_cache /tmp/cache 1:2
Restart=always
RestartSec=3s
 
[Install]
WantedBy=multi-user.target
```

Each `--zone` takes three values: the name that purge requests refer to, the `path` and the `levels` set by `proxy_cache_path` or `fastcgi_cache_path`. It can be used more than once.

Defining a zone is strongly recommended. Without it, anyone who can reach the socket may name **any** directory as the cache path, which combined with the key `*` deletes that whole directory.

Run the following commands,

```bash
sudo systemctl daemon-reload
sudo systemctl start nginx-cache-purge
sudo systemctl status nginx-cache-purge

sudo systemctl enable nginx-cache-purge
```

#### Edit Nginx' Configuration File

Assume we want to put the cache in `/tmp/cache`.

```nginx
http {
    ...

    map $request_method $is_purge {                                                             
        default   0;
        PURGE     1;
    }

    proxy_cache_path /tmp/cache levels=1:2 keys_zone=my_cache:10m;
    proxy_cache_key $scheme$request_uri;

    server {
        ...

        location / {
            if ($is_purge) {
                rewrite ^ /nginx-cache-purge last;
            }

            proxy_cache my_cache;
            proxy_pass upstream;
            include proxy_params;
        }

        location = /nginx-cache-purge {
            internal;

            proxy_set_header X-Cache-Zone my_cache;
            # $request_uri is still the URI of the original request here
            proxy_set_header X-Cache-Key  $scheme$request_uri;

            proxy_pass http://unix:/tmp/nginx-cache-purge.sock;
        }
    }
}
```

Passing the key in a header is recommended, because a header value is taken exactly as it is. A key may therefore contain `?`, `&` or `%`, which a query string cannot carry safely.

Remember to add your access authentication mechanisms to prevent strangers from purging your cache. And note that the cache key should not contain `$proxy_host` because it will be empty when the request is in `proxy_pass http://unix:...`.

After finishing the settings:

* Request `PURGE /path/to/abc` to purge the cache from `GET /path/to/abc`.
* Request `PURGE /path/to/*` to purge all caches from `GET /path/to/**/*`.
* Request `PURGE /path/to/*/foo/*/bar` to purge caches from `GET /path/to/**/foo/**/bar`.

If the service successfully removes any cache, it will respond the HTTP status code **200**. If no cache needs to be removed, it will respond the HTTP status code **202**. If the request is malformed, it will respond the HTTP status code **400**.

#### Request Fields

Every field can be set either as a request header or as a field in the query of the `/` endpoint URL. A header wins over the query.

| Header | Query field | Description |
| ------ | ----------- | ----------- |
| `X-Cache-Zone` | `zone` | The name of a zone defined by `--zone`. |
| `X-Cache-Path` | `cache_path` | The `path` set by `proxy_cache_path` or `fastcgi_cache_path`. Rejected if the server defines any zone. |
| `X-Cache-Levels` | `levels` | The `levels` set by `proxy_cache_path` or `fastcgi_cache_path`. Rejected if the server defines any zone. |
| `X-Cache-Key` | `key` | The key to purge. Required. |
| `X-Remove-First` | `remove_first` | Strip this prefix from the `key`, for example `/purge`. It does not affect the excluded keys. |
| `X-Exclude-Key` | `exclude_keys` | Exclude the keys that match this pattern from the purging process. It can be used more than once. |

#### Using the Query Instead of Headers

If you would rather keep the whole request in the URL, note the two rules below.

```nginx
location / {
    if ($is_purge) {
        set $my_cache_key $scheme$request_uri;

        proxy_pass http://unix:/tmp/nginx-cache-purge.sock;

        # the trailing "?" stops Nginx from appending the arguments of the original request again
        rewrite ^ "/?zone=my_cache&key=$my_cache_key?" break;
    }

    proxy_cache my_cache;
    proxy_pass upstream;
    include proxy_params;
}
```

1. **`key` has to be the last field.** Its value runs to the end of the query, so that a key containing `&` or `?` is not cut short.
2. **The values are not percent-decoded.** Nginx builds its cache key from the raw `$request_uri`, so `%20` and `+` have to reach this service unchanged. A field value therefore cannot contain `&`; use the matching header when you need one.

### No Service

If we want to use `nginx-cache-purge` CLI with [lua-nginx-module](https://github.com/openresty/lua-nginx-module), instead of running the service in the background.

We can choose to disable the default features to obtain a much smaller executable binary.

```bash
cargo install nginx-cache-purge --no-default-features
```

## Upgrading from 0.4

* A query field is no longer percent-decoded, and `key` now runs to the end of the query. If your `rewrite` puts `key` in the middle, move it to the end, and append a `?` to the replacement string so that Nginx does not add the arguments of the original request again.
* A key pattern without a trailing `*` now has to match the whole cache key. `*/help` used to behave like `*/help*`.
* A debug build no longer refuses to remove anything. Use `--dry-run` instead.
* `-e` now takes one key at a time, so use it once per key.

## License

[MIT](LICENSE)