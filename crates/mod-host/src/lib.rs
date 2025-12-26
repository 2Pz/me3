#![feature(fn_traits)]
#![feature(fn_ptr_trait)]
#![feature(tuple_trait)]
#![feature(unboxed_closures)]

use std::{
    fs::OpenOptions,
    io::stdout,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, OnceLock,
    },
};

use eyre::OptionExt;
use me3_binary_analysis::{fd4_step::Fd4StepTables, rtti};
use me3_env::TelemetryVars;
use me3_launcher_attach_protocol::{AttachConfig, AttachRequest, AttachResult, Attachment};
use me3_mod_host_assets::mapping::VfsOverrideMapping;
use me3_telemetry::TelemetryConfig;
use tracing::{error, info, warn, Span};
use windows::core::{s, w};
use windows::Win32::{
    Foundation::{GetLastError, ERROR_INVALID_PARAMETER},
    Globalization::CP_UTF8,
    System::{
        Console::SetConsoleOutputCP,
        LibraryLoader::{GetModuleHandleW, GetProcAddress},
        SystemServices::{DLL_PROCESS_ATTACH, DLL_PROCESS_DETACH},
        Threading::{GetCurrentProcess, GetProcessAffinityMask},
    },
};

use crate::{
    deferred::{defer_init, Deferred},
    executable::Executable,
    host::{game_properties, ModHost},
};

mod alloc_hooks;
mod asset_hooks;
mod debugger;
mod deferred;
mod detour;
mod executable;
mod filesystem;
mod host;
mod native;
mod savefile;
mod skip_logos;

static INSTANCE: OnceLock<usize> = OnceLock::new();
static mut TELEMETRY_INSTANCE: OnceLock<me3_telemetry::Telemetry> = OnceLock::new();
static AFFINITY_LOGGED_ONCE: AtomicBool = AtomicBool::new(false);

fn install_affinity_workaround() -> Result<(), eyre::Error> {
    type SetThreadAffinityMaskFn =
        unsafe extern "system" fn(windows::Win32::Foundation::HANDLE, usize) -> usize;

    // Prefer kernel32: on some systems `SetThreadAffinityMask` is not exported from kernelbase.
    let kernel32 = unsafe { GetModuleHandleW(w!("kernel32.dll")).ok() };
    let kernelbase = unsafe { GetModuleHandleW(w!("kernelbase.dll")).ok() };

    let set_thread_affinity_mask: SetThreadAffinityMaskFn = unsafe {
        let p = kernel32
            .and_then(|m| GetProcAddress(m, s!("SetThreadAffinityMask")))
            .or_else(|| kernelbase.and_then(|m| GetProcAddress(m, s!("SetThreadAffinityMask"))))
            .ok_or_eyre("SetThreadAffinityMask not found (tried kernel32.dll, kernelbase.dll)")?;

        std::mem::transmute(p)
    };

    ModHost::get_attached()
        .hook(set_thread_affinity_mask)
        .with_closure(move |thread, requested_mask, trampoline| unsafe {
            let prev = trampoline(thread, requested_mask);
            if prev != 0 {
                return prev;
            }

            if GetLastError() != ERROR_INVALID_PARAMETER {
                return 0;
            }

            let mut process_mask: usize = 0;
            let mut system_mask: usize = 0;
            if GetProcessAffinityMask(
                GetCurrentProcess(),
                &mut process_mask,
                &mut system_mask,
            )
            .is_err()
                || process_mask == 0
            {
                return 0;
            }

            let patched_mask = match requested_mask & process_mask {
                0 => process_mask,
                m => m,
            };

            let prev2 = trampoline(thread, patched_mask);
            if prev2 != 0 && !AFFINITY_LOGGED_ONCE.swap(true, Ordering::Relaxed) {
                warn!("thread affinity workaround active: process has restricted CPU affinity (e.g. CPU0 disabled)");
            }
            prev2
        })
        .install()?;

    info!("installed thread affinity workaround");
    Ok(())
}

dll_syringe::payload_procedure! {
    fn me_attach(request: AttachRequest) -> AttachResult {
        if request.config.suspend {
            debugger::suspend_for_debugger();
        }

        on_attach(request)
    }
}

#[cfg(coverage)]
#[unsafe(no_mangle)]
#[allow(non_upper_case_globals)]
static __llvm_profile_runtime: i32 = 1;

#[cfg(coverage)]
unsafe extern "C" {
    fn __llvm_profile_write_file() -> i32;
    fn __llvm_profile_initialize_file();
}

fn on_attach(request: AttachRequest) -> AttachResult {
    let _ = unsafe { SetConsoleOutputCP(CP_UTF8) };
    me3_telemetry::install_error_handler();

    let attach_config = Arc::new(request.config);

    let telemetry_vars: TelemetryVars = me3_env::deserialize_from_env()?;
    let telemetry_log_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&telemetry_vars.log_file_path)?;

    let telemetry_config = TelemetryConfig::default()
        .enabled(telemetry_vars.enabled)
        .with_console_writer(stdout)
        .with_file_writer(telemetry_log_file)
        .capture_panics(true);

    let telemetry_guard = me3_telemetry::install(telemetry_config);

    #[allow(static_mut_refs)]
    let _ = unsafe { TELEMETRY_INSTANCE.set(telemetry_guard) };

    let result = me3_telemetry::with_root_span("host", "attach", move || {
        info!("Beginning host attach");

        // SAFETY: process is still suspended.
        let exe = unsafe { Executable::new() };

        match exe.version() {
            Ok(ver) => info!("Attaching to {ver}"),
            Err(e) => warn!("error" = %e, "could not detect game version"),
        }

        ModHost::new(&attach_config).attach();

        // Elden Ring (as far as we know): can crash during startup if CPU0 is excluded from the
        // process affinity (e.g. Process Lasso). This installs a small workaround.
        if let Err(e) = install_affinity_workaround() {
            warn!("error" = %e, "failed to install affinity workaround");
        }

        dearxan(&attach_config)?;

        skip_logos::attach_override(attach_config.clone(), exe)?;

        game_properties::attach_override(attach_config.clone(), exe)?;

        if !attach_config.start_online {
            game_properties::start_offline();
        }

        let mut override_mapping = VfsOverrideMapping::new()?;

        override_mapping.scan_directories(attach_config.packages.iter())?;
        savefile::attach_override(&attach_config, &mut override_mapping)?;

        let override_mapping = Arc::new(override_mapping);

        filesystem::attach_override(override_mapping.clone())?;

        info!("Host successfully attached");

        let before_main_result = Arc::new(Mutex::new(None));

        defer_init(Span::current(), Deferred::BeforeMain, {
            let result = before_main_result.clone();
            let attach_config = attach_config.clone();
            move || *result.lock().unwrap() = Some(before_game_main(attach_config, exe))
        })?;

        defer_init(Span::current(), Deferred::AfterMain, move || {
            let result = after_game_main(attach_config, exe, override_mapping, move || {
                before_main_result
                    .lock()
                    .unwrap()
                    .take()
                    .ok_or_eyre("`before_game_main` did not run?")?
            });

            if let Err(e) = result {
                error!("error" = &*e, "deferred attach failed!")
            }
        })?;

        info!("Deferred me3 attach");

        Ok(Attachment)
    })?;

    Ok(result)
}

fn before_game_main(attach_config: Arc<AttachConfig>, exe: Executable) -> Result<(), eyre::Error> {
    if attach_config.mem_patch {
        alloc_hooks::hook_system_allocator(&attach_config, exe)?;
    }

    for native in &attach_config.early_natives {
        ModHost::get_attached().load_native(&native.path, &native.initializer)?;
    }

    Ok(())
}

fn after_game_main<R: FnOnce() -> Result<(), eyre::Error>>(
    attach_config: Arc<AttachConfig>,
    exe: Executable,
    override_mapping: Arc<VfsOverrideMapping>,
    before_main_result: R,
) -> Result<(), eyre::Error> {
    before_main_result()?;

    let class_map = Arc::new(rtti::classes(exe)?);
    let step_tables = Fd4StepTables::from_initialized_data(exe)?;

    if attach_config.mem_patch {
        alloc_hooks::hook_heap_allocators(&attach_config, exe, &class_map)?;
    }

    savefile::oversized_regulation_fix(
        attach_config.clone(),
        exe,
        &step_tables,
        override_mapping.clone(),
    )?;

    let first_delayed_offset = attach_config
        .natives
        .iter()
        .enumerate()
        .filter_map(|(idx, native)| native.initializer.is_some().then_some(idx))
        .next()
        .unwrap_or(attach_config.natives.len());

    let (immediate, delayed) = attach_config.natives.split_at(first_delayed_offset);

    for native in immediate {
        if let Err(e) = ModHost::get_attached().load_native(&native.path, &native.initializer) {
            warn!(
                error = &*e,
                path = %native.path.display(),
                "failed to load native mod",
            );

            if !native.optional {
                return Err(e);
            }
        }
    }

    let delayed = delayed.to_vec();
    std::thread::spawn(move || {
        for native in delayed {
            if let Err(e) = ModHost::get_attached().load_native(&native.path, &native.initializer) {
                warn!(
                    error = &*e,
                    path = %native.path.display(),
                    "failed to load native mod",
                );

                if !native.optional {
                    panic!("{:#?}", e);
                }
            }
        }
    });

    asset_hooks::attach_override(
        attach_config,
        exe,
        class_map,
        &step_tables,
        override_mapping,
    )
    .map_err(|e| {
        e.wrap_err("failed to attach asset override hooks; no files will be overridden")
    })?;

    Ok(())
}

fn dearxan(attach_config: &AttachConfig) -> Result<(), eyre::Error> {
    if !ModHost::get_attached().disable_arxan {
        return Ok(());
    }

    info!(
        "game" = %attach_config.game,
        "attach_config.disable_arxan" = attach_config.disable_arxan,
        "will attempt to disable Arxan code protection",
    );

    defer_init(Span::current(), Deferred::BeforeMain, || {
        info!("dearxan::disabler::neuter_arxan finished")
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn DllMain(instance: usize, reason: u32, _: *mut usize) -> i32 {
    match reason {
        DLL_PROCESS_ATTACH => {
            #[cfg(coverage)]
            unsafe {
                __llvm_profile_initialize_file()
            };

            let _ = INSTANCE.set(instance);
        }
        DLL_PROCESS_DETACH => {
            #[cfg(coverage)]
            unsafe {
                __llvm_profile_write_file()
            };

            // FIXME: this panics on process exit, either on thread creation or accessing a thread
            // local. Ideally, it should be called at an earlier point.
            //
            // The crash handler (when re-added) should call flush instead.
            //
            // std::thread::spawn(|| {
            //     #[allow(static_mut_refs)]
            //     let telemetry = unsafe { TELEMETRY_INSTANCE.take() };
            //     drop(telemetry);
            // });
        }
        _ => {}
    }

    1
}
