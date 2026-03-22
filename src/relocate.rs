use iced_x86::{
    BlockEncoder, BlockEncoderOptions, Decoder, DecoderOptions, FlowControl, InstructionBlock,
};

use crate::error::{Error, Result};

pub struct Relocated {
    pub bytes: Vec<u8>,
    pub stolen_len: usize,
}

pub fn relocate(
    code: &[u8],
    original_rip: u64,
    new_rip: u64,
    min_bytes: usize,
) -> Result<Relocated> {
    let mut decoder = Decoder::with_ip(64, code, original_rip, DecoderOptions::NONE);
    let mut instructions = Vec::new();
    let mut stolen_len = 0usize;

    while stolen_len < min_bytes {
        let instr = decoder.decode();
        if instr.is_invalid() {
            return Err(Error::RelocationFailed);
        }

        stolen_len += instr.len();
        instructions.push(instr);

        if matches!(
            instr.flow_control(),
            FlowControl::Return
                | FlowControl::UnconditionalBranch
                | FlowControl::Exception
                | FlowControl::Interrupt
        ) && stolen_len < min_bytes
        {
            return Err(Error::InsufficientSpace {
                need: min_bytes,
                have: stolen_len,
            });
        }
    }

    let block = InstructionBlock::new(&instructions, new_rip);
    let result = BlockEncoder::encode(64, block, BlockEncoderOptions::NONE)
        .map_err(|_| Error::RelocationFailed)?;

    Ok(Relocated {
        bytes: result.code_buffer,
        stolen_len,
    })
}
