// @generated
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct Message {
    #[prost(string, tag="1")]
    pub say: ::prost::alloc::string::String,
}
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct Response {
    #[prost(string, tag="1")]
    pub say: ::prost::alloc::string::String,
}
include!("helloworld.tonic.rs");
// @@protoc_insertion_point(module)
