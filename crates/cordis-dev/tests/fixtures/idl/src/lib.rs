//! DOC-15 §7、ADR-0039：命令生成的接口在正式 SDK 上执行行为验收。

pub mod kv {
    include!(concat!(env!("CORDIS_IDL_GENERATED"), "/kv.rs"));
}

pub mod types {
    include!(concat!(env!("CORDIS_IDL_GENERATED"), "/types.rs"));
}

#[cfg(test)]
mod behavior;
