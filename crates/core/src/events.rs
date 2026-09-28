//! A tiny broadcast event bus (plan §4.5): every catalog mutation goes
//! through a `Command`, which emits an event here. View models subscribe and
//! invalidate their cached state; background threads never touch UI state
//! directly, they only ever publish.

use std::sync::Mutex;

use crossbeam_channel::{Receiver, Sender, unbounded};

use crate::ids::{CollectionId, ImportId, PhotoId};

/// Broadcasts values of type `E` to every live subscriber.
///
/// Cloning `E` happens once per subscriber per publish, so keep event
/// payloads small (ids and enums, not whole rows).
pub struct EventBus<E: Clone> {
    subscribers: Mutex<Vec<Sender<E>>>,
}

impl<E: Clone> std::fmt::Debug for EventBus<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventBus")
            .field("subscribers", &self.subscriber_count())
            .finish()
    }
}

impl<E: Clone> Default for EventBus<E> {
    fn default() -> Self {
        Self {
            subscribers: Mutex::new(Vec::new()),
        }
    }
}

impl<E: Clone> EventBus<E> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a new subscriber. The UI drains its receiver once per
    /// frame; background threads must never block on it.
    pub fn subscribe(&self) -> Receiver<E> {
        let (tx, rx) = unbounded();
        #[allow(clippy::unwrap_used)]
        // a poisoned mutex means a prior panic; there's no safe recovery
        self.subscribers.lock().unwrap().push(tx);
        rx
    }

    /// Sends `event` to every subscriber, dropping any whose receiver was
    /// closed.
    pub fn publish(&self, event: E) {
        #[allow(clippy::unwrap_used)]
        let mut subs = self.subscribers.lock().unwrap();
        subs.retain(|tx| tx.send(event.clone()).is_ok());
    }

    pub fn subscriber_count(&self) -> usize {
        #[allow(clippy::unwrap_used)]
        self.subscribers.lock().unwrap().len()
    }
}

/// Fields on a `photos` row (or its joined metadata) that changed, so a
/// subscriber can skip re-fetching data it doesn't display.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PhotoField {
    Rating,
    Flag,
    ColorLabel,
    Keywords,
    Metadata,
    DevelopSettings,
    Orientation,
}

/// Catalog-wide events, published after a `Command` commits (plan §4.5).
#[derive(Debug, Clone)]
pub enum CatalogEvent {
    PhotosChanged {
        ids: Vec<PhotoId>,
        fields: Vec<PhotoField>,
    },
    PhotosAdded {
        ids: Vec<PhotoId>,
        import_id: Option<ImportId>,
    },
    PhotosRemoved {
        ids: Vec<PhotoId>,
    },
    CollectionsChanged {
        ids: Vec<CollectionId>,
    },
    ImportProgress {
        import_id: ImportId,
        scanned: u32,
        imported: u32,
        total_hint: Option<u32>,
    },
    ImportFinished {
        import_id: ImportId,
        imported: u32,
    },
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn subscribers_receive_published_events() {
        let bus: EventBus<CatalogEvent> = EventBus::new();
        let rx1 = bus.subscribe();
        let rx2 = bus.subscribe();

        bus.publish(CatalogEvent::PhotosRemoved {
            ids: vec![PhotoId::new(1)],
        });

        for rx in [&rx1, &rx2] {
            match rx.try_recv().expect("event") {
                CatalogEvent::PhotosRemoved { ids } => assert_eq!(ids, vec![PhotoId::new(1)]),
                other => panic!("unexpected event: {other:?}"),
            }
        }
    }

    #[test]
    fn dropped_receiver_is_pruned_on_next_publish() {
        let bus: EventBus<CatalogEvent> = EventBus::new();
        {
            let _rx = bus.subscribe();
            assert_eq!(bus.subscriber_count(), 1);
        }
        bus.publish(CatalogEvent::PhotosRemoved { ids: vec![] });
        assert_eq!(bus.subscriber_count(), 0);
    }
}
