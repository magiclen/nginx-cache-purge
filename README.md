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
(curl -s https://api.github.com/repos/magiclen/nginx-cache-purge/releases/latest | sed -r -n 's/.*"browser_download_url": *"(.*\/nginx-cache-purge_'$(uname -m)')".*/\1/p' | wget -i -) && sudo mv nginx-cache-purge_$(uname -m) /usr/local/bin/nginx-cache-purge && sudo chmod +x /usr/local/bin/nginx-cache-purge

# sudo rm /usr/local/bin/nginx-cache-purge
```

### Nginx + lua-nginx-module + Nginx Cache Purge

Assume we have already put the executable file `nginx-cache-purge` in `/usr/local/bin/`.

```nginx
http {
    ...

    map $request_method $is_purge {                                                             
        default   0;
        PURGE     1;
    }

    proxy_cache_path /path/to/cache levels=1:2 keys_zone=my_cache:10m;
    proxy_cache_key $scheme$proxy_host$request_uri;

    server {
        ...

        location / {
            if ($is_purge) {
                set $my_cache_key $scheme$proxy_host$request_uri;

                content_by_lua_block {
                    local ngx_pipe = require "ngx.pipe"
                    local exitStatus, err = ngx_pipe.spawn({"/usr/local/bin/nginx-cache-purge", "/path/to/cache", "1:2", ngx.var.my_cache_key})
                    if err then
                        ngx.log("purge error: ", err)
                    end
                    if exitStatus == 0 then
                        ngx.exit(ngx.HTTP_OK)
                    else
                        ngx.exit(ngx.HTTP_BAD_REQUEST)
                    end
                } 
            }

            proxy_pass upstream;
            include proxy_params;
        }
    }
}
```

Remember to add your access authentication mechanisms to prevent strangers from purging your cache.

After finishing the settings:

* Request `PURGE /path/to/abc` to purge the cache from `GET /path/to/abc`.
* Request `PURGE /path/to/*` to purge all caches from `GET /path/to/**/*`.

## License

[MIT](LICENSE)
