CARGO = cargo

README.md: src/lib.rs
	$(CARGO) readme > $@
%/README.md: %/src/lib.rs
	pushd $*; $(CARGO) readme > README.md; popd

readme: README.md tourniquet-celery/README.md tourniquet-tonic/README.md

.PHONY: readme
