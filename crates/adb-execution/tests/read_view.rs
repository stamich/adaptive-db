//! The read view of a query: snapshot and pinned schema epoch travel from the context to every
//! data-source call.
use adb_core::{CommitTs, SchemaEpoch};
use adb_execution::{ExecutionContext, ReadView};

/// A context without an epoch reads the latest schemas; a pinned one carries its epoch.
#[test]
fn contexts_produce_their_read_view() {
    let mut context = ExecutionContext::new(CommitTs(7));
    assert_eq!(context.read_view(), ReadView::from(CommitTs(7)));
    assert_eq!(context.read_view().schema_epoch, None);
    context.schema_epoch = Some(SchemaEpoch(3));
    assert_eq!(
        context.read_view(),
        ReadView::pinned(CommitTs(7), SchemaEpoch(3))
    );
}
