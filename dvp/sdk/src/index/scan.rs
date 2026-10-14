use zolana_interface::instruction::instruction_data::transact::MessageData;
use zolana_transaction::{instructions::transact::OutputSlot, ShieldedTransaction};

/// The messages of `tx` tagged `tag`, with their message index.
pub(crate) fn tagged_messages<'a>(
    tx: &'a ShieldedTransaction,
    tag: &'a [u8; 32],
) -> impl Iterator<Item = (usize, &'a MessageData)> {
    tx.messages
        .iter()
        .enumerate()
        .filter(move |(_, message)| &message.view_tag == tag)
}

/// The output slots of `tx` tagged `tag`.
pub(crate) fn tagged_outputs<'a>(
    tx: &'a ShieldedTransaction,
    tag: &'a [u8; 32],
) -> impl Iterator<Item = &'a OutputSlot> {
    tx.output_slots
        .iter()
        .filter(move |slot| &slot.view_tag == tag)
}
