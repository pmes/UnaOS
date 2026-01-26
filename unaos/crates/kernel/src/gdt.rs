use x86_64::structures::gdt::{GlobalDescriptorTable, Descriptor, SegmentSelector};
use x86_64::structures::tss::TaskStateSegment;
use x86_64::VirtAddr;
use lazy_static::lazy_static;

pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;

lazy_static! {
    static ref TSS: TaskStateSegment = {
        let mut tss = TaskStateSegment::new();
        tss.interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] = {
            const STACK_SIZE: usize = 4096 * 5;
            static mut STACK: [u8; STACK_SIZE] = [0; STACK_SIZE];

            let stack_start = VirtAddr::from_ptr(&raw const STACK);
            stack_start + STACK_SIZE
        };
        tss
    };
}

lazy_static! {
    static ref GDT: (GlobalDescriptorTable, Selectors) = {
        let mut gdt = GlobalDescriptorTable::new();
        let code_selector = gdt.add_entry(Descriptor::kernel_code_segment());
        let tss_selector = gdt.add_entry(Descriptor::tss_segment(&TSS));
        (gdt, Selectors { code_selector, tss_selector })
    };
}

struct Selectors {
    code_selector: SegmentSelector,
    tss_selector: SegmentSelector,
}

pub fn init() {
    use x86_64::instructions::tables::load_tss;
    use x86_64::instructions::segmentation::{CS, DS, ES, FS, GS, SS, Segment};

    GDT.0.load();
    unsafe {
        // 1. Reload the Code Segment (CS) with our new GDT offset
        CS::set_reg(GDT.1.code_selector);

        // 2. Load the Task State Segment (TSS) for the Safe Stack
        load_tss(GDT.1.tss_selector);

        // 3. THE FIX: Sanitize the Data Segments
        // We set SS, DS, ES, FS, and GS to the NULL selector (0).
        // This stops the CPU from checking the old bootloader segments (like 16)
        // against our new GDT, preventing the General Protection Fault.
        let null_selector = SegmentSelector::new(0, x86_64::PrivilegeLevel::Ring0);
        SS::set_reg(null_selector);
        DS::set_reg(null_selector);
        ES::set_reg(null_selector);
        FS::set_reg(null_selector);
        GS::set_reg(null_selector);
    }
}
