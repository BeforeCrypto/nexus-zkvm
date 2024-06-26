use std::{
    fs::File,
    io::BufReader,
    path::{Path, PathBuf},
};

use anyhow::Context;
use ark_serialize::CanonicalDeserialize;
use nexus_config::{
    vm::{NovaImpl, ProverImpl, VmConfig},
    Config,
};
use nexus_prover::types::{ComPCDNode, ComPP, ComProof, IVCProof, PCDNode, ParPP, SeqPP};
use nexus_tools_dev::command::common::{
    prove::{CommonProveArgs, LocalProveArgs},
    public_params::format_params_file,
    spartan_key::format_key_file,
    VerifyArgs,
};

use crate::{command::cache_path, LOG_TARGET};

use super::jolt;

pub fn handle_command(args: VerifyArgs) -> anyhow::Result<()> {
    let VerifyArgs {
        file,
        compressed,
        prover_args: LocalProveArgs { k, pp_file, prover_impl: nova_impl, .. },
        key_file,
        common_args,
    } = args;

    let vm_config = VmConfig::from_env()?;
    if compressed {
        verify_proof_compressed(&file, k.unwrap_or(vm_config.k), pp_file, key_file)
    } else {
        verify_proof(
            &file,
            k.unwrap_or(vm_config.k),
            nova_impl.unwrap_or(vm_config.prover),
            common_args,
            pp_file,
        )
    }
}

fn verify_proof_compressed(
    path: &Path,
    k: usize,
    pp_file: Option<PathBuf>,
    key_file: Option<PathBuf>,
) -> anyhow::Result<()> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);

    let pp_path = match pp_file {
        Some(path) => path,
        None => {
            let pp_file_name = format_params_file(NovaImpl::ParallelCompressible, k);
            let cache_path = cache_path()?;

            cache_path.join(pp_file_name)
        }
    }
    .to_str()
    .context("path is not utf-8")?
    .to_owned();

    let key_path = match key_file {
        Some(path) => path,
        None => {
            let key_file_name = format_key_file(k);
            let cache_path = cache_path()?;

            cache_path.join(key_file_name)
        }
    }
    .to_str()
    .context("path is not utf-8")?
    .to_owned();

    let mut term = nexus_tui::TerminalHandle::new_enabled();
    let mut ctx = term
        .context("Verifying compressed")
        .on_step(move |_step| "proof".into());
    let mut _guard = Default::default();

    let result = {
        let proof = ComProof::deserialize_compressed(reader)?;
        let params = nexus_prover::pp::gen_or_load(false, k, &pp_path, None)?;
        let key = nexus_prover::key::gen_or_load_key(false, &key_path, Some(&pp_path), None)?;

        _guard = ctx.display_step();
        nexus_prover::verify_compressed(&key, &params, &proof).map_err(anyhow::Error::from)
    };

    match result {
        Ok(_) => {
            drop(_guard);

            tracing::info!(
                target: LOG_TARGET,
                "Compressed proof is valid",
            );
        }
        Err(err) => {
            _guard.abort();

            tracing::error!(
                target: LOG_TARGET,
                err = ?err,
                ?k,
                "Compressed proof is invalid",
            );
            std::process::exit(1);
        }
    }

    Ok(())
}

fn verify_proof(
    path: &Path,
    k: usize,
    prover: ProverImpl,
    prove_args: CommonProveArgs,
    pp_file: Option<PathBuf>,
) -> anyhow::Result<()> {
    // handle jolt separately
    let nova_impl = match prover {
        ProverImpl::Jolt => return jolt::verify(path, prove_args),
        ProverImpl::Nova(nova_impl) => nova_impl,
    };

    let file = File::open(path)?;
    let reader = BufReader::new(file);

    let path = match pp_file {
        Some(path) => path,
        None => {
            let pp_file_name = format_params_file(nova_impl, k);
            let cache_path = cache_path()?;

            cache_path.join(pp_file_name)
        }
    }
    .to_str()
    .context("path is not utf8")?
    .to_owned();

    let mut term = nexus_tui::TerminalHandle::new_enabled();
    let mut ctx = term.context("Verifying").on_step(move |_step| {
        match nova_impl {
            NovaImpl::Parallel => "root",
            NovaImpl::ParallelCompressible => "root",
            NovaImpl::Sequential => "proof",
        }
        .into()
    });
    let mut _guard = Default::default();

    let result = match nova_impl {
        NovaImpl::Parallel => {
            let root = PCDNode::deserialize_compressed(reader)?;
            let params: ParPP = nexus_prover::pp::gen_or_load(false, k, &path, None)?;

            _guard = ctx.display_step();
            root.verify(&params).map_err(anyhow::Error::from)
        }
        NovaImpl::ParallelCompressible => {
            let root = ComPCDNode::deserialize_compressed(reader)?;
            let params: ComPP = nexus_prover::pp::gen_or_load(false, k, &path, None)?;

            _guard = ctx.display_step();
            root.verify(&params).map_err(anyhow::Error::from)
        }
        NovaImpl::Sequential => {
            let proof = IVCProof::deserialize_compressed(reader)?;
            let params: SeqPP = nexus_prover::pp::gen_or_load(false, k, &path, None)?;

            _guard = ctx.display_step();
            proof
                .verify(&params, proof.step_num() as usize)
                .map_err(anyhow::Error::from)
        }
    };

    match result {
        Ok(_) => {
            drop(_guard);

            tracing::info!(
                target: LOG_TARGET,
                "Proof is valid",
            );
        }
        Err(err) => {
            _guard.abort();

            tracing::error!(
                target: LOG_TARGET,
                err = ?err,
                ?k,
                %nova_impl,
                "Proof is invalid",
            );
            std::process::exit(1);
        }
    }
    Ok(())
}
