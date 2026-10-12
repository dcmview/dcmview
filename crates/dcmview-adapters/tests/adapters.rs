//! The adapters through their public API: what each format reads and
//! writes, byte for byte where bytes are the contract, what every report
//! says, and what an import does with input nobody vouches for.

mod adapters {
    mod embed_adapter;
    mod embed_rows;
    mod native;
    mod support;
    mod untrusted;
}
