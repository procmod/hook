use iced_x86::{
    BlockEncoder, BlockEncoderOptions, Decoder, DecoderError, DecoderOptions, FlowControl,
    Instruction, InstructionBlock,
};

use crate::error::{Error, Result};
use crate::jump;

/// Where a hook's code lives and how much of the target its patch overwrites.
pub struct Site {
    pub target: u64,
    pub trampoline: u64,
    pub capacity: usize,
    pub patch_len: usize,
}

/// Trampoline code: the relocated stolen instructions followed by a jump back.
pub struct Layout {
    pub code: Vec<u8>,
    pub stolen_len: usize,
}

/// Bytes of prologue that decoding may need: a patch of `patch_len` bytes can end
/// one byte into an instruction of the maximum x86 length.
pub fn max_stolen_len(patch_len: usize) -> usize {
    patch_len - 1 + 15
}

pub fn layout(prologue: &[u8], site: &Site) -> Result<Layout> {
    let instructions = steal(prologue, site)?;
    let stolen_len = instructions.iter().map(Instruction::len).sum::<usize>();
    let block = InstructionBlock::new(&instructions, site.trampoline);
    let mut code = BlockEncoder::encode(64, block, BlockEncoderOptions::NONE)
        .map_err(|_| Error::RelocationFailed)?
        .code_buffer;
    let jump_back_at = site.trampoline + code.len() as u64;
    let jump_back = jump::encode_rel32(jump_back_at, site.target + stolen_len as u64)
        .ok_or(Error::RelocationFailed)?;
    code.extend_from_slice(&jump_back);
    if code.len() > site.capacity {
        return Err(Error::RelocationFailed);
    }
    Ok(Layout { code, stolen_len })
}

fn steal(prologue: &[u8], site: &Site) -> Result<Vec<Instruction>> {
    let mut decoder = Decoder::with_ip(64, prologue, site.target, DecoderOptions::NONE);
    let mut instructions = Vec::new();
    let mut stolen_len = 0usize;
    while stolen_len < site.patch_len {
        let instruction = decoder.decode();
        if instruction.is_invalid() {
            return Err(match decoder.last_error() {
                DecoderError::NoMoreBytes => Error::PrologueUnreadable {
                    address: (site.target as usize) + prologue.len(),
                },
                _ => Error::RelocationFailed,
            });
        }
        stolen_len += instruction.len();
        instructions.push(instruction);
        if ends_flow(&instruction) && stolen_len < site.patch_len {
            return Err(Error::InsufficientSpace {
                need: site.patch_len,
                have: stolen_len,
            });
        }
    }
    Ok(instructions)
}

fn ends_flow(instruction: &Instruction) -> bool {
    matches!(
        instruction.flow_control(),
        FlowControl::Return
            | FlowControl::UnconditionalBranch
            | FlowControl::IndirectBranch
            | FlowControl::Exception
            | FlowControl::Interrupt
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site(patch_len: usize) -> Site {
        Site {
            target: 0x1000_0000,
            trampoline: 0x1001_0000,
            capacity: 64,
            patch_len,
        }
    }

    #[test]
    fn steals_whole_instructions_and_jumps_back() {
        // push rbp; mov rbp, rsp; sub rsp, 0x20; ret
        let prologue = [0x55, 0x48, 0x89, 0xE5, 0x48, 0x83, 0xEC, 0x20, 0xC3];
        let layout = layout(&prologue, &site(jump::REL32_LEN)).unwrap();
        assert_eq!(layout.stolen_len, 8);
        assert_eq!(&layout.code[..8], &prologue[..8]);
        let jump_back = jump::encode_rel32(0x1001_0008, 0x1000_0008).unwrap();
        assert_eq!(&layout.code[8..], &jump_back);
    }

    #[test]
    fn relocates_rip_relative_operands() {
        // mov rax, [rip + 0x100]; ret
        let prologue = [0x48, 0x8B, 0x05, 0x00, 0x01, 0x00, 0x00, 0xC3];
        let layout = layout(&prologue, &site(jump::REL32_LEN)).unwrap();
        let mut decoder = Decoder::with_ip(64, &layout.code, 0x1001_0000, DecoderOptions::NONE);
        let relocated = decoder.decode();
        assert_eq!(relocated.memory_displacement64(), 0x1000_0000 + 7 + 0x100);
    }

    #[test]
    fn reports_a_prologue_cut_short() {
        let prologue = [0x90, 0x90, 0x90];
        assert!(matches!(
            layout(&prologue, &site(jump::REL32_LEN)),
            Err(Error::PrologueUnreadable {
                address: 0x1000_0003
            })
        ));
    }

    #[test]
    fn reports_an_instruction_cut_short() {
        // nop; mov eax, imm32 missing its last byte
        let prologue = [0x90, 0xB8, 0x2A, 0x00, 0x00];
        assert!(matches!(
            layout(&prologue, &site(jump::REL32_LEN)),
            Err(Error::PrologueUnreadable { .. })
        ));
    }

    #[test]
    fn rejects_functions_that_return_too_early() {
        let prologue = [0x90, 0xC3, 0xCC, 0xCC, 0xCC, 0xCC];
        assert!(matches!(
            layout(&prologue, &site(jump::REL32_LEN)),
            Err(Error::InsufficientSpace { need: 5, have: 2 })
        ));
    }

    #[test]
    fn rejects_indirect_jumps_before_the_patch_ends() {
        // jmp rax
        let prologue = [0xFF, 0xE0, 0xCC, 0xCC, 0xCC, 0xCC];
        assert!(matches!(
            layout(&prologue, &site(jump::REL32_LEN)),
            Err(Error::InsufficientSpace { .. })
        ));
    }

    #[test]
    fn rejects_invalid_instructions() {
        let prologue = [0x06, 0x90, 0x90, 0x90, 0x90, 0x90];
        assert!(matches!(
            layout(&prologue, &site(jump::REL32_LEN)),
            Err(Error::RelocationFailed)
        ));
    }

    #[test]
    fn rejects_code_that_exceeds_capacity() {
        let prologue = [0x90; 16];
        let site = Site {
            capacity: 8,
            ..site(jump::REL32_LEN)
        };
        assert!(matches!(
            layout(&prologue, &site),
            Err(Error::RelocationFailed)
        ));
    }

    #[test]
    fn steals_enough_for_an_absolute_jump() {
        let prologue = [0x90; 28];
        let layout = layout(&prologue, &site(jump::ABS64_LEN)).unwrap();
        assert_eq!(layout.stolen_len, jump::ABS64_LEN);
        assert_eq!(max_stolen_len(jump::ABS64_LEN), 28);
    }
}
