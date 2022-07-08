# We use tonic 0.7 + prost 0.10, which correspond to protoc-gen-prost 0.1 and protoc-gen-tonic 0.1
PROTOC_PLUGINS = protoc-gen-prost-0.1.4 protoc-gen-tonic-0.1.1

protoc-gen-%:
	cargo install --version $(word 2,$(subst -, ,$*)) protoc-gen-$(word 1,$(subst -, ,$*))

gen/helloworld.rs: $(PROTOC_PLUGINS) proto/hello.proto proto/types.proto
	protoc --prost_out=gen --tonic_out=gen -I proto hello.proto types.proto

.PHONY: $(PROTOC_PLUGINS) gen/helloworld.rs
