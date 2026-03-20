use std::{fs, io::Cursor, path::PathBuf, sync::OnceLock};

use halo2_base::halo2_proofs::{
    halo2curves::bn256::Bn256,
    poly::{commitment::Params, kzg::commitment::ParamsKZG},
};

use crate::data::ParameterSet;

#[cfg(not(debug_assertions))]
mod embedded {
    pub(super) const BYTES_6: &[u8] = include_bytes!("../../../fixtures/params/kzg_bn254_6.srs");
    pub(super) const BYTES_8: &[u8] = include_bytes!("../../../fixtures/params/kzg_bn254_8.srs");
    pub(super) const BYTES_9: &[u8] = include_bytes!("../../../fixtures/params/kzg_bn254_9.srs");
    pub(super) const BYTES_14: &[u8] = include_bytes!("../../../fixtures/params/kzg_bn254_14.srs");
    pub(super) const BYTES_21: &[u8] = include_bytes!("../../../fixtures/params/kzg_bn254_21.srs");
}

static PARAMS_6: OnceLock<ParamsKZG<Bn256>> = OnceLock::new();
static PARAMS_8: OnceLock<ParamsKZG<Bn256>> = OnceLock::new();
static PARAMS_9: OnceLock<ParamsKZG<Bn256>> = OnceLock::new();
static PARAMS_14: OnceLock<ParamsKZG<Bn256>> = OnceLock::new();
static PARAMS_21: OnceLock<ParamsKZG<Bn256>> = OnceLock::new();

#[cfg(debug_assertions)]
fn get_params_path(k: u32) -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .join("../../fixtures/params")
        .join(format!("kzg_bn254_{k}.srs"))
}

fn load_from_bytes(bytes: &[u8]) -> ParamsKZG<Bn256> {
    ParamsKZG::read(&mut Cursor::new(bytes)).unwrap()
}

#[cfg(debug_assertions)]
fn load_from_fs(k: u32) -> ParamsKZG<Bn256> {
    let path = get_params_path(k);
    let bytes = fs::read(&path)
        .unwrap_or_else(|_| panic!("Failed to read params file: {}", path.display()));
    load_from_bytes(&bytes)
}

pub(crate) fn load_params(params: ParameterSet) -> &'static ParamsKZG<Bn256> {
    #[cfg(debug_assertions)]
    return match params {
        ParameterSet::Six => PARAMS_6.get_or_init(|| load_from_fs(6)),
        ParameterSet::Eight => PARAMS_8.get_or_init(|| load_from_fs(8)),
        ParameterSet::Nine => PARAMS_9.get_or_init(|| load_from_fs(9)),
        ParameterSet::Fourteen => PARAMS_14.get_or_init(|| load_from_fs(14)),
        ParameterSet::TwentyOne => PARAMS_21.get_or_init(|| load_from_fs(21)),
    };

    #[cfg(not(debug_assertions))]
    return match params {
        ParameterSet::Six => PARAMS_6.get_or_init(|| load_from_bytes(embedded::BYTES_6)),
        ParameterSet::Eight => PARAMS_8.get_or_init(|| load_from_bytes(embedded::BYTES_8)),
        ParameterSet::Nine => PARAMS_9.get_or_init(|| load_from_bytes(embedded::BYTES_9)),
        ParameterSet::Fourteen => PARAMS_14.get_or_init(|| load_from_bytes(embedded::BYTES_14)),
        ParameterSet::TwentyOne => PARAMS_21.get_or_init(|| load_from_bytes(embedded::BYTES_21)),
    };
}
