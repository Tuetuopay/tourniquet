CARGO = cargo

README.md: src/lib.rs
	$(CARGO) readme > $@
