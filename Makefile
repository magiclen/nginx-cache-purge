all: ./target/x86_64-unknown-linux-musl/release/nginx-cache-purge

./target/x86_64-unknown-linux-musl/release/nginx-cache-purge: $(shell find . -type f -iname '*.rs' -o -name 'Cargo.toml' | sed 's/ /\\ /g')
	cargo build --release --target x86_64-unknown-linux-musl
	strip ./target/x86_64-unknown-linux-musl/release/nginx-cache-purge
	
install:
	$(MAKE)
	sudo cp ./target/x86_64-unknown-linux-musl/release/nginx-cache-purge /usr/local/bin/nginx-cache-purge
	sudo chown root. /usr/local/bin/nginx-cache-purge
	sudo chmod 0755 /usr/local/bin/nginx-cache-purge

uninstall:
	sudo rm /usr/local/bin/nginx-cache-purges

test:
	cargo test --verbose

clean:
	cargo clean
