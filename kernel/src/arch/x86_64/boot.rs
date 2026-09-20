use aether_bootinfo::{
    BootInfo, FramebufferInfo, MemoryRegion, MemoryRegionKind, MAX_MEMORY_REGIONS,
};
use core::sync::atomic::{AtomicU8, Ordering};
use limine::request::{FramebufferRequest, HhdmRequest, MemmapRequest, StackSizeRequest};
use limine::{BaseRevision, RequestsEndMarker, RequestsStartMarker};
use spin::Once;

use crate::arch::x86_64::serial;

#[used]
#[unsafe(link_section = ".limine_requests")]
static BASE_REVISION: BaseRevision = BaseRevision::new();

#[used]
#[unsafe(link_section = ".limine_requests")]
static HHDM_REQUEST: HhdmRequest = HhdmRequest::new();

#[used]
#[unsafe(link_section = ".limine_requests")]
static MEMORY_MAP_REQUEST: MemmapRequest = MemmapRequest::new();

#[used]
#[unsafe(link_section = ".limine_requests")]
static FRAMEBUFFER_REQUEST: FramebufferRequest = FramebufferRequest::new();

#[used]
#[unsafe(link_section = ".limine_requests")]
static STACK_SIZE_REQUEST: StackSizeRequest = StackSizeRequest::new(1024 * 1024);

#[used]
#[unsafe(link_section = ".limine_requests_start")]
static REQUESTS_START: RequestsStartMarker = RequestsStartMarker::new();

#[used]
#[unsafe(link_section = ".limine_requests_end")]
static REQUESTS_END: RequestsEndMarker = RequestsEndMarker::new();

static BOOT_INFO: Once<BootInfo> = Once::new();
static BOOT_STAGE: AtomicU8 = AtomicU8::new(BootStage::Entry as u8);

pub fn enter() -> ! {
    mark_boot_stage(BootStage::Entry);
    serial::init();
    log_boot_stage("kernel entry reached");

    if !BASE_REVISION.is_supported() {
        mark_boot_stage(BootStage::RevisionRejected);
        serial::write_line("AetherOS: unsupported Limine base revision");
        crate::core::panic::halt_forever();
    }

    mark_boot_stage(BootStage::RevisionAccepted);
    log_boot_stage("Limine base revision accepted");

    let boot_info = BOOT_INFO.call_once(collect_boot_info);

    mark_boot_stage(BootStage::BootInfoReady);
    log_boot_summary(boot_info);
    mark_boot_stage(BootStage::KernelInit);
    log_boot_stage("transferring control to generic kernel init");
    crate::run(boot_info)
}

fn collect_boot_info() -> BootInfo {
    mark_boot_stage(BootStage::CollectingBootInfo);
    log_boot_stage("collecting boot info");

    let hhdm_response = HHDM_REQUEST
        .response()
        .expect("Limine did not provide an HHDM response");
    mark_boot_stage(BootStage::HhdmReady);
    log_boot_stage("HHDM response collected");

    let memory_map_response = MEMORY_MAP_REQUEST
        .response()
        .expect("Limine did not provide a memory map response");
    mark_boot_stage(BootStage::MemoryMapReady);
    log_boot_stage("memory map response collected");

    let entries = memory_map_response.entries();
    let _ = serial::write_fmt(format_args!(
        "AetherOS: boot [{}] normalizing {} memory regions\n",
        current_boot_stage_label(),
        entries.len().min(MAX_MEMORY_REGIONS)
    ));
    let mut regions = [MemoryRegion::EMPTY; MAX_MEMORY_REGIONS];

    for (index, entry) in entries.iter().take(MAX_MEMORY_REGIONS).enumerate() {
        let _ = serial::write_fmt(format_args!(
            "AetherOS: boot [{}] memory region {}\n",
            current_boot_stage_label(),
            index
        ));
        regions[index] = MemoryRegion::new(
            entry.base,
            entry.length,
            classify_memory_region(entry.type_),
        );
    }
    mark_boot_stage(BootStage::RegionsMapped);
    log_boot_stage("memory regions normalized");

    let framebuffer = FRAMEBUFFER_REQUEST
        .response()
        .and_then(|response| response.framebuffers().first().copied())
        .map(|framebuffer| {
            FramebufferInfo::new(
                framebuffer.address() as u64,
                framebuffer.width as u32,
                framebuffer.height as u32,
                framebuffer.pitch as u32,
                framebuffer.bpp,
            )
        });
    mark_boot_stage(BootStage::FramebufferReady);
    log_boot_stage(if framebuffer.is_some() {
        "framebuffer response collected"
    } else {
        "framebuffer unavailable"
    });

    BootInfo::new(
        hhdm_response.offset,
        memory_map_response.entries().len(),
        framebuffer,
        regions,
    )
}

fn classify_memory_region(entry_type: u64) -> MemoryRegionKind {
    match entry_type {
        0 => MemoryRegionKind::Usable,
        5 => MemoryRegionKind::Reclaimable,
        6 => MemoryRegionKind::Kernel,
        7 => MemoryRegionKind::Framebuffer,
        1..=4 => MemoryRegionKind::Reserved,
        _ => MemoryRegionKind::Unknown,
    }
}

pub fn current_boot_stage_label() -> &'static str {
    BootStage::from_u8(BOOT_STAGE.load(Ordering::Acquire)).label()
}

fn mark_boot_stage(stage: BootStage) {
    BOOT_STAGE.store(stage as u8, Ordering::Release);
}

fn log_boot_stage(message: &str) {
    let _ = serial::write_fmt(format_args!(
        "AetherOS: boot [{}] {}\n",
        current_boot_stage_label(),
        message
    ));
}

fn log_boot_summary(boot_info: &BootInfo) {
    let _ = serial::write_fmt(format_args!(
        "AetherOS: boot [{}] hhdm={:#x} memmap_entries={} region_capacity={} framebuffer={}\n",
        current_boot_stage_label(),
        boot_info.hhdm_offset,
        boot_info.memory_map_entries,
        MAX_MEMORY_REGIONS,
        if boot_info.framebuffer.is_some() {
            "yes"
        } else {
            "no"
        }
    ));
}

#[repr(u8)]
#[derive(Clone, Copy)]
enum BootStage {
    Entry = 0,
    RevisionAccepted = 1,
    RevisionRejected = 2,
    CollectingBootInfo = 3,
    HhdmReady = 4,
    MemoryMapReady = 5,
    RegionsMapped = 6,
    FramebufferReady = 7,
    BootInfoReady = 8,
    KernelInit = 9,
}

impl BootStage {
    const fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::RevisionAccepted,
            2 => Self::RevisionRejected,
            3 => Self::CollectingBootInfo,
            4 => Self::HhdmReady,
            5 => Self::MemoryMapReady,
            6 => Self::RegionsMapped,
            7 => Self::FramebufferReady,
            8 => Self::BootInfoReady,
            9 => Self::KernelInit,
            _ => Self::Entry,
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::Entry => "entry",
            Self::RevisionAccepted => "revision-accepted",
            Self::RevisionRejected => "revision-rejected",
            Self::CollectingBootInfo => "collecting-boot-info",
            Self::HhdmReady => "hhdm-ready",
            Self::MemoryMapReady => "memory-map-ready",
            Self::RegionsMapped => "regions-mapped",
            Self::FramebufferReady => "framebuffer-ready",
            Self::BootInfoReady => "boot-info-ready",
            Self::KernelInit => "kernel-init",
        }
    }
}
