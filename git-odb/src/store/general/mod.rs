#![allow(missing_docs, unused, dead_code)]

use arc_swap::ArcSwap;
use std::ops::Deref;
use std::path::PathBuf;
use std::sync::atomic::AtomicUsize;

/// This effectively acts like a handle but exists to be usable from the actual `crate::Handle` implementation which adds caches on top.
/// Each store is quickly cloned and contains thread-local state for shared packs.
#[derive(Clone)]
pub struct Handle<S>
where
    S: Deref<Target = Store> + Clone,
{
    state: S,
}

pub struct Store {
    /// The source directory from which all content is loaded, and the central write lock for use when a directory refresh is needed.
    path: parking_lot::Mutex<PathBuf>,

    /// A list of indices keeping track of which slots are filled with data. These are usually, but not always, consecutive.
    pub(crate) index: ArcSwap<store::SlotMapIndex>,

    /// The amount of handles that would prevent us from unloading packs or indices
    pub(crate) num_handles_stable: AtomicUsize,
    /// The amount of handles that don't affect our ability to compact our internal data structures or unload packs or indices.
    pub(crate) num_handles_unstable: AtomicUsize,
}

mod find {
    use git_hash::oid;
    use git_object::Data;
    use git_pack::cache::DecodeEntry;
    use git_pack::data::entry::Location;
    use git_pack::index::Entry;
    use std::ops::Deref;

    impl<S> crate::pack::Find for super::Handle<S>
    where
        S: Deref<Target = super::Store> + Clone,
    {
        type Error = crate::compound::find::Error;

        fn contains(&self, id: impl AsRef<oid>) -> bool {
            todo!()
        }

        fn try_find_cached<'a>(
            &self,
            id: impl AsRef<oid>,
            buffer: &'a mut Vec<u8>,
            pack_cache: &mut impl DecodeEntry,
        ) -> Result<Option<(Data<'a>, Option<Location>)>, Self::Error> {
            todo!()
        }

        fn location_by_oid(&self, id: impl AsRef<oid>, buf: &mut Vec<u8>) -> Option<Location> {
            todo!()
        }

        fn index_iter_by_pack_id(&self, pack_id: u32) -> Option<Box<dyn Iterator<Item = Entry> + '_>> {
            todo!()
        }

        fn entry_by_location(&self, location: &Location) -> Option<git_pack::find::Entry<'_>> {
            todo!()
        }
    }
}

mod init {
    use crate::general::store::SlotMapIndex;
    use arc_swap::ArcSwap;
    use git_features::threading::OwnShared;
    use std::path::PathBuf;
    use std::sync::atomic::AtomicUsize;
    use std::sync::Arc;

    impl super::Store {
        pub fn at(objects_dir: impl Into<PathBuf>) -> std::io::Result<Self> {
            let objects_dir = objects_dir.into();
            if !objects_dir.is_dir() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Other, // TODO: use NotADirectory when stabilized
                    format!("'{}' wasn't a directory", objects_dir.display()),
                ));
            }
            Ok(super::Store {
                path: parking_lot::Mutex::new(objects_dir),
                index: ArcSwap::new(Arc::new(SlotMapIndex::default())),
                num_handles_stable: Default::default(),
                num_handles_unstable: Default::default(),
            })
        }

        pub fn to_handle(self: &OwnShared<Self>) -> super::Handle<OwnShared<super::Store>> {
            super::Handle { state: self.clone() }
        }
    }
}

mod store {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// An id to refer to an index file or a multipack index file
    pub type IndexId = usize;
    pub(crate) type StateId = usize;

    /// A way to indicate which pack indices we have seen already and which of them are loaded, along with an idea
    /// of whether stored `PackId`s are still usable.
    #[derive(Clone, Default)]
    pub struct SlotIndexMarker {
        /// The generation the `loaded_until_index` belongs to. Indices of different generations are completely incompatible.
        /// This value changes once the internal representation is compacted, something that may happen only if there is no handle
        /// requiring stable pack indices.
        generation: u8,
        /// A unique id identifying the index state as well as all loose databases we have last observed.
        /// If it changes in any way, the value is different.
        state_id: StateId,
    }

    /// A way to load and refer to a pack uniquely, namespaced by their indexing mechanism, aka multi-pack or not.
    pub struct PackId {
        /// Note that if `multipack_index = None`, this index is corresponding to the index id.
        /// So a pack is always identified by its corresponding index.
        /// If it is a multipack index, this is the id / offset of the pack in the `multipack_index`.
        pub(crate) index: IndexId,
        pub(crate) multipack_index: Option<IndexId>,
    }

    /// An index that changes only if the packs directory changes and its contents is re-read.
    #[derive(Default)]
    pub struct SlotMapIndex {
        /// The index into the slot map at which we expect an index or pack file. Neither of these might be loaded yet.
        pub(crate) slot_indices: Vec<usize>,
        /// A static value that doesn't ever change for a particular clone of this index.
        pub(crate) generation: u8,
        /// The number of indices loaded thus far when the index of the slot map was last examined, which can change as new indices are loaded
        /// in parallel.
        /// Shared across SlotMapIndex instances of the same generation.
        pub(crate) next_index_to_load: Arc<AtomicUsize>,
        /// Incremented by one up to `slot_indices.len()` once index was actually loaded. If a load failed, there will be no increment.
        /// Shared across SlotMapIndex instances of the same generation.
        pub(crate) loaded_indices: Arc<AtomicUsize>,
        /// A list of loose object databases as resolved by their alternates file in the `object_directory`. The first entry is this objects
        /// directory loose file database. All other entries are the loose stores of alternates.
        /// It's in an Arc to be shared to Handles, but not to be shared across SlotMapIndices
        pub(crate) loose_dbs: Arc<Vec<crate::loose::Store>>,
    }

    impl SlotMapIndex {
        pub(crate) fn state_id(self: &Arc<SlotMapIndex>) -> StateId {
            // We let the loaded indices take part despite not being part of our own snapshot.
            // This is to account for indices being loaded in parallel without actually changing the snapshot itself.
            (Arc::as_ptr(&self.loose_dbs) as usize ^ Arc::as_ptr(self) as usize)
                * (self.loaded_indices.load(Ordering::SeqCst) + 1)
        }
    }

    /// Note that this is a snapshot of SlotMapIndex, even though some internal values are shared, it's for sharing to callers, not among
    /// versions of the SlotMapIndex
    impl From<&Arc<SlotMapIndex>> for SlotIndexMarker {
        fn from(v: &Arc<SlotMapIndex>) -> Self {
            SlotIndexMarker {
                generation: v.generation,
                state_id: v.state_id(),
            }
        }
    }
}

pub mod handle {
    use crate::general::store;
    use std::sync::Arc;

    mod multi_index {
        // TODO: replace this one with an actual implementation of a multi-pack index.
        pub type File = ();
    }

    pub enum SingleOrMultiIndex {
        Single {
            index: Arc<git_pack::index::File>,
            data: Option<Arc<git_pack::data::File>>,
        },
        Multi {
            index: Arc<multi_index::File>,
            data: Vec<Option<Arc<git_pack::data::File>>>,
        },
    }

    pub struct IndexLookup {
        file: SingleOrMultiIndex,
        id: store::IndexId,
    }

    pub struct IndexForObjectInPack {
        /// The internal identifier of the pack itself, which either is referred to by an index or a multi-pack index.
        pack_id: store::PackId,
        /// The index of the object within the pack
        object_index_in_pack: u32,
    }

    pub(crate) mod index_lookup {
        use crate::general::{handle, store};
        use git_hash::oid;
        use std::sync::Arc;

        impl handle::IndexLookup {
            /// See if the oid is contained in this index, and return its full id for lookup possibly alongside its data file if already
            /// loaded.
            /// If it is not loaded, ask it to be loaded and put it into the returned mutable option for safe-keeping.
            fn lookup(
                &mut self,
                object_id: &oid,
            ) -> Option<(handle::IndexForObjectInPack, &mut Option<Arc<git_pack::data::File>>)> {
                let id = self.id;
                match &mut self.file {
                    handle::SingleOrMultiIndex::Single { index, data } => {
                        index.lookup(object_id).map(|object_index_in_pack| {
                            (
                                handle::IndexForObjectInPack {
                                    pack_id: store::PackId {
                                        index: id,
                                        multipack_index: None,
                                    },
                                    object_index_in_pack,
                                },
                                data,
                            )
                        })
                    }
                    handle::SingleOrMultiIndex::Multi { index, data } => {
                        todo!("find respective pack and return it as &mut Option<>")
                    }
                }
            }
        }
    }
}

pub mod load_indices {
    use crate::general::{handle, store};

    /// Define how packs will be refreshed when all indices are loaded, which is useful if a lot of objects are missing.
    #[derive(Clone, Copy)]
    pub enum RefreshMode {
        /// Check for new or changed pack indices (and pack data files) when the last known index is loaded.
        /// During runtime we will keep pack indices stable by never reusing them, however, there is the option for
        /// clearing internal caches which is likely to change pack ids and it will trigger unloading of packs as they are missing on disk.
        AfterAllIndicesLoaded,
        /// Use this if you expect a lot of missing objects that shouldn't trigger refreshes even after all packs are loaded.
        /// This comes at the risk of not learning that the packs have changed in the mean time.
        Never,
    }

    use crate::general::store::StateId;
    use std::sync::Arc;

    pub(crate) enum Outcome {
        /// Drop all data and fully replace it with `indices`.
        /// This happens if we have witnessed a generational change invalidating all of our ids and causing currently loaded
        /// indices and maps to be dropped.
        Replace {
            indices: Vec<handle::IndexLookup>, // should probably be SmallVec to get around most allocations
            loose_dbs: Arc<Vec<crate::loose::Store>>,
            marker: store::SlotIndexMarker, // use to show where the caller left off last time
        },
        /// No new indices to look at, caller should give up
        NoMoreIndices,
    }

    impl super::Store {
        pub(crate) fn load_next_indices(
            &self,
            refresh_mode: RefreshMode,
            marker: Option<store::SlotIndexMarker>,
        ) -> std::io::Result<Outcome> {
            let index = self.index.load();
            if index.loose_dbs.is_empty() {
                // TODO: figure out what kind of refreshes we need. This one loads in the initial slot map, but I think this cost is paid
                //       in full during instantiation.
                return self.consolidate_with_disk_state(index.state_id());
            }
            //
            // Ok(match marker {
            //     Some(marker) => {
            //         if marker.generation != index.generation {
            //             self.collect_replace_outcome()
            //         } else if marker.state_id == index.state_id() {
            //             match refresh_mode {
            //                 store::RefreshMode::Never => load_indices::Outcome::NoMoreIndices,
            //                 store::RefreshMode::AfterAllIndicesLoaded => return self.refresh(),
            //             }
            //         } else {
            //             self.collect_replace_outcome()
            //         }
            //     }
            //     None => self.collect_replace_outcome(),
            // })
            todo!()
        }

        /// refresh and possibly clear out our existing data structures, causing all pack ids to be invalidated.
        fn consolidate_with_disk_state(&self, seen: StateId) -> std::io::Result<Outcome> {
            let objects_directory = self.path.lock();
            if seen != self.index.load().state_id() {
                return todo!();
            }
            let mut db_paths = crate::alternate::resolve(&*objects_directory)
                .map_err(|err| std::io::Error::new(std::io::ErrorKind::Other, err))?;
            // These are in addition to our objects directory
            db_paths.insert(0, objects_directory.clone());
            todo!()
        }
    }
}
