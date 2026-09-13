Changelog
====================

## Unreleased

### Added

* Both commands accept `--no-wildcard` to reject purge keys containing `*`. Wildcard purges remain enabled by default. Rejected requests return HTTP **400**, and the CLI returns exit status **1**. The option also applies to dry runs, but does not restrict excluded keys. ([#9](https://github.com/magiclen/nginx-cache-purge/issues/9))
* Both commands accept `--scan` to find all files with an exact cache key, including Nginx `Vary` variants. Without it, exact purges keep using the direct file path.
* The service accepts `--max-concurrent-purges N` to limit the number of purges running at once. Other requests wait; no limit is added by default.

### Fixed

* Exact excluded keys now protect `Vary` variants as well as the main cache file during wildcard purges and scans.
* Starting a second server no longer removes an active server's socket. Stale sockets can still be recovered, and cleanup checks that the socket belongs to this server.
* Shutdown now waits for active HTTP requests to finish sending their responses, and cleans up the socket even when serving fails.
* Builds without the `service` feature no longer show service command examples in their help.

### Changed

* Full purges now pass file paths through a bounded queue instead of collecting every path before starting. Large subdirectories are handed to separate workers.

## 0.5.0

### Fixed

* The key of a cache file could be read from the wrong place, or not be read at all. Nginx writes a binary header in front of the key, and that header may contain a line feed. The old code just skipped the first line, so it often looked at the wrong bytes: a cache which should be purged was kept, and sometimes the read ended with `Error: Kind(InvalidData)`. The key is now found by searching for the `KEY: ` line. ([#1](https://github.com/magiclen/nginx-cache-purge/issues/1))
* The `key` field of a query was cut at the first `&`. It now runs to the end of the query, so a key which contains `&` or `?` stays complete. Because of that, `key` has to be the last field. ([#5](https://github.com/magiclen/nginx-cache-purge/issues/5))
* The fields of a query are no longer percent-decoded. Nginx builds its cache key from the raw `$request_uri`, so `%20` and `+` have to reach this program unchanged. ([#5](https://github.com/magiclen/nginx-cache-purge/issues/5))

### Added

* Every request field can also be sent as a request header: `X-Cache-Zone`, `X-Cache-Path`, `X-Cache-Levels`, `X-Cache-Key`, `X-Remove-First` and `X-Exclude-Key`. A header value is taken exactly as it is, so a key may contain `?`, `&` or `%` without any escaping. A header wins over the query.
* The `start` command has a new `--zone NAME PATH LEVELS` option, which can be used more than once. Once a zone is defined, a request may only name a zone, and can no longer choose a cache path of its own.
* Both commands have a new `--dry-run` option. It prints what would be removed and removes nothing.
* The service now answers a purge request at any path and with any method.
* The service now stops on `SIGINT` and `SIGTERM`, and removes its socket file before it ends.

### Changed

* Purging with a wildcard now reads and removes cache files with several threads, so a big cache directory takes much less time than before. ([#7](https://github.com/magiclen/nginx-cache-purge/issues/7))
* A key pattern without a trailing `*` now has to match the whole cache key. `*/help` used to work like `*/help*`.
* `levels` may now be empty, as it may be omitted in Nginx. Every cache file then sits directly in the cache directory.
* `-e` (`--exclude-keys`) now takes one key at a time. Use it once per key.
* The socket file is now created with mode `0660` instead of `0777`. Nginx and this service therefore need a user or a group which they share.
* The service answers the HTTP status code **400** when a request field is missing or invalid, and **500** when the purge itself fails.
* A debug build now removes files just like a release build does. Use `--dry-run` if nothing should be removed.
* The `purge` command no longer needs Tokio, so `--no-default-features` builds a much smaller executable file.
* The minimum supported Rust version is now 1.89, and the 2024 edition is used.
