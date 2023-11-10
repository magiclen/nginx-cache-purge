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
nginx-cache-purge purge /path/to/cache 1:2 http/blog/    # Purge the cache with the key "http/blog/" in the "cache zone" whose "path" is /path/to/cache, "levels" is 1:2
nginx-cache-purge purge /path/to/cache 1:1:1 http/blog*  # Purge the caches with the key which has "http/blog" as its prefix in the "cache zone" whose "path" is /path/to/cache, "levels" is 1:1:1
nginx-cache-purge purge /path/to/cache 1 */help*         # Purge the caches with the key which contains the substring "/help" in the "cache zone" whose "path" is /path/to/cache, "levels" is 1
nginx-cache-purge purge /path/to/cache 2 *               # Purge all caches in the "cache zone" whose "path" is /path/to/cache, "levels" is 2
nginx-cache-purge start                                  # Start a server which listens on "/tmp/nginx-cache-purge.sock" to handle purge requests
nginx-cache-purge start /run/nginx-cache-purge.sock      # Start a server which listens on "/run/nginx-cache-purge.sock" to handle purge requests

Usage: nginx-cache-purge <COMMAND>

Commands:
  purge  Purge the cache immediately [aliases: p]
  start  Start a server to handle purge requests [aliases: s]
  help   Print this message or the help of the given subcommand(s)

Options:
  -h, --help     Print help
  -V, --version  Print version
```

If the `purge` command successfully removes any cache, it returns the exit status **0**. If no cache needs to be removed, it returns the exit status **44**.

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

ExecStart=/usr/local/bin/nginx-cache-purge start
Restart=always
RestartSec=3s
 
[Install]
WantedBy=multi-user.target
```

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
                set $my_cache_key $scheme$request_uri;
            
                proxy_pass http://unix:/tmp/nginx-cache-purge.sock;
                
                rewrite ^ /?cache_path=/tmp/cache&levels=1:2&key=$my_cache_key break;
            }

            proxy_cache my_cache;
            proxy_pass upstream;
            include proxy_params;
        }
    }
}
```

Remember to add your access authentication mechanisms to prevent strangers from purging your cache. And note that the cache key should not contain `$proxy_host` because it will be empty when the request is in `proxy_pass http://unix:...`.

After finishing the settings:

* Request `PURGE /path/to/abc` to purge the cache from `GET /path/to/abc`.
* Request `PURGE /path/to/*` to purge all caches from `GET /path/to/**/*`.
* Request `PURGE /path/to/*/foo/*/bar` to purge caches from `GET /path/to/**/foo/**/bar`.

If the service successfully removes any cache, it will respond the HTTP status code **200**. If no cache needs to be removed, it will respond the HTTP status code **202**.

The `remove_first` field can be set to the query of the `/` endpoint URL, allowing the exclusion of the prefix from the request path of the `key`.

### No Service

If we want to use `nginx-cache-purge` CLI with [lua-nginx-module](https://github.com/openresty/lua-nginx-module), instead of running the service in the background.

We can choose to disable the default features to obtain a much smaller executable binary.

```bash
cargo install nginx-cache-purge --no-default-features
```

## License

[MIT](LICENSE)