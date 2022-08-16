# We use tonic 0.8 + prost 0.11, which correspond to protoc-gen-prost 0.2 and protoc-gen-tonic 0.2
PROTOC_PLUGINS = protoc-gen-prost-0.2.0 protoc-gen-tonic-0.2.0

all: gen/helloworld.rs

protoc-gen-%:
	cargo install --version $(word 2,$(subst -, ,$*)) protoc-gen-$(word 1,$(subst -, ,$*))

gen/helloworld.rs: $(PROTOC_PLUGINS) proto/hello.proto proto/types.proto
	protoc --prost_out=gen --tonic_out=gen -I proto hello.proto types.proto

.PHONY: $(PROTOC_PLUGINS) gen/helloworld.rs
