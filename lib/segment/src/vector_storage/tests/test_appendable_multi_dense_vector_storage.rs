use std::path::Path;
use std::sync::Arc;

use atomic_refcell::AtomicRefCell;
use common::counter::hardware_counter::HardwareCounterCell;
use common::types::PointOffsetType;
use common::validation::MAX_MULTIVECTOR_FLATTENED_LEN;
use rstest::rstest;
use tempfile::Builder;

use crate::data_types::vectors::{
    MultiDenseVectorInternal, QueryVector, TypedMultiDenseVectorRef, VectorElementType, VectorRef,
};
use crate::fixtures::payload_context_fixture::FixtureIdTracker;
use crate::id_tracker::IdTrackerSS;
use crate::index::hnsw_index::point_scorer::FilteredScorer;
use crate::types::{Distance, MultiVectorConfig};
use crate::vector_storage::common::CHUNK_SIZE;
use crate::vector_storage::multi_dense::appendable_mmap_multi_dense_vector_storage::open_appendable_memmap_multi_vector_storage_full;
use crate::vector_storage::multi_dense::volatile_multi_dense_vector_storage::new_volatile_multi_dense_vector_storage;
use crate::vector_storage::{
    DEFAULT_STOPPED, MultiVectorStorage, VectorStorage, VectorStorageEnum,
};

#[derive(Clone, Copy)]
enum MultiDenseStorageType {
    #[cfg(feature = "rocksdb")]
    RocksDbFloat,
    AppendableMmapFloat,
}

fn multi_points_fixtures(vec_count: usize, vec_dim: usize) -> Vec<MultiDenseVectorInternal> {
    let mut multis: Vec<MultiDenseVectorInternal> = Vec::new();
    for i in 0..vec_count {
        let value = i as f32;
        // hardcoded 5 inner vectors
        let vectors = vec![
            vec![value; vec_dim],
            vec![value; vec_dim],
            vec![value; vec_dim],
            vec![value; vec_dim],
            vec![value; vec_dim],
        ];
        let multi = MultiDenseVectorInternal::try_from(vectors).unwrap();
        multis.push(multi);
    }
    multis
}

fn do_test_delete_points(vector_dim: usize, vec_count: usize, storage: &mut VectorStorageEnum) {
    let points = multi_points_fixtures(vec_count, vector_dim);

    let delete_mask = [false, false, true, true, false];

    let id_tracker: Arc<AtomicRefCell<IdTrackerSS>> =
        Arc::new(AtomicRefCell::new(FixtureIdTracker::new(points.len())));

    let borrowed_id_tracker = id_tracker.borrow_mut();

    let hw_counter = HardwareCounterCell::new();

    // Insert all points
    for (i, vec) in points.iter().enumerate() {
        storage
            .insert_vector(i as PointOffsetType, vec.into(), &hw_counter)
            .unwrap();
    }
    // Check that all points are inserted
    for (i, vec) in points.iter().enumerate() {
        let stored_vec = storage.get_vector(i as PointOffsetType);
        let multi_dense: TypedMultiDenseVectorRef<_> = stored_vec.as_vec_ref().try_into().unwrap();
        assert_eq!(multi_dense.to_owned(), vec.clone());
    }
    // Check that all points are inserted #2
    {
        let orig_iter = points.iter().flat_map(|multivec| multivec.multi_vectors());
        match storage as &VectorStorageEnum {
            #[cfg(feature = "rocksdb")]
            VectorStorageEnum::DenseSimple(_)
            | VectorStorageEnum::DenseSimpleByte(_)
            | VectorStorageEnum::DenseSimpleHalf(_) => unreachable!(),
            #[cfg(test)]
            VectorStorageEnum::DenseVolatile(_)
            | VectorStorageEnum::DenseVolatileByte(_)
            | VectorStorageEnum::DenseVolatileHalf(_) => unreachable!(),
            VectorStorageEnum::DenseMemmap(_)
            | VectorStorageEnum::DenseMemmapByte(_)
            | VectorStorageEnum::DenseMemmapHalf(_) => unreachable!(),
            VectorStorageEnum::DenseAppendableMemmap(_)
            | VectorStorageEnum::DenseAppendableMemmapByte(_)
            | VectorStorageEnum::DenseAppendableMemmapHalf(_) => unreachable!(),
            #[cfg(feature = "rocksdb")]
            VectorStorageEnum::SparseSimple(_) => unreachable!(),
            VectorStorageEnum::SparseMmap(_) => unreachable!(),
            #[cfg(test)]
            VectorStorageEnum::SparseVolatile(_) => unreachable!(),
            #[cfg(feature = "rocksdb")]
            VectorStorageEnum::MultiDenseSimple(v) => {
                for (orig, vec) in orig_iter.zip(v.iterate_inner_vectors()) {
                    assert_eq!(orig, vec);
                }
            }
            #[cfg(feature = "rocksdb")]
            VectorStorageEnum::MultiDenseSimpleByte(_)
            | VectorStorageEnum::MultiDenseSimpleHalf(_) => unreachable!(),
            VectorStorageEnum::MultiDenseVolatile(v) => {
                for (orig, vec) in orig_iter.zip(v.iterate_inner_vectors()) {
                    assert_eq!(orig, vec);
                }
            }
            VectorStorageEnum::MultiDenseVolatileByte(_)
            | VectorStorageEnum::MultiDenseVolatileHalf(_) => unreachable!(),
            VectorStorageEnum::MultiDenseAppendableMemmap(v) => {
                for (orig, vec) in orig_iter.zip(v.iterate_inner_vectors()) {
                    assert_eq!(orig, vec);
                }
            }
            VectorStorageEnum::MultiDenseAppendableMemmapByte(_)
            | VectorStorageEnum::MultiDenseAppendableMemmapHalf(_) => unreachable!(),
            VectorStorageEnum::DenseAppendableInRam(_)
            | VectorStorageEnum::DenseAppendableInRamByte(_)
            | VectorStorageEnum::DenseAppendableInRamHalf(_) => unreachable!(),
            VectorStorageEnum::MultiDenseAppendableInRam(_)
            | VectorStorageEnum::MultiDenseAppendableInRamByte(_)
            | VectorStorageEnum::MultiDenseAppendableInRamHalf(_) => unreachable!(),
        };
    }

    // Delete select number of points
    delete_mask
        .into_iter()
        .enumerate()
        .filter(|(_, d)| *d)
        .for_each(|(i, _)| {
            storage.delete_vector(i as PointOffsetType).unwrap();
        });
    assert_eq!(
        storage.deleted_vector_count(),
        2,
        "2 vectors must be deleted"
    );
    let vector: Vec<Vec<f32>> = vec![vec![2.0; vector_dim]];
    let query = QueryVector::Nearest(vector.try_into().unwrap());
    let scorer =
        FilteredScorer::new_for_test(query, storage, borrowed_id_tracker.deleted_point_bitslice());
    let closest = scorer
        .peek_top_iter(&mut [0, 1, 2, 3, 4].iter().cloned(), 5, &DEFAULT_STOPPED)
        .unwrap();
    assert_eq!(closest.len(), 3, "must have 3 vectors, 2 are deleted");
    assert_eq!(closest[0].idx, 4);
    assert_eq!(closest[1].idx, 1);
    assert_eq!(closest[2].idx, 0);
    drop(scorer);

    // Delete 1, redelete 2
    storage.delete_vector(1 as PointOffsetType).unwrap();
    storage.delete_vector(2 as PointOffsetType).unwrap();
    assert_eq!(
        storage.deleted_vector_count(),
        3,
        "3 vectors must be deleted"
    );

    let vector: Vec<Vec<f32>> = vec![vec![1.0; vector_dim]];
    let query = QueryVector::Nearest(vector.try_into().unwrap());
    let scorer =
        FilteredScorer::new_for_test(query, storage, borrowed_id_tracker.deleted_point_bitslice());
    let closest = scorer
        .peek_top_iter(&mut [0, 1, 2, 3, 4].iter().cloned(), 5, &DEFAULT_STOPPED)
        .unwrap();
    assert_eq!(closest.len(), 2, "must have 2 vectors, 3 are deleted");
    assert_eq!(closest[0].idx, 4);
    assert_eq!(closest[1].idx, 0);
    drop(scorer);

    // Delete all
    storage.delete_vector(0 as PointOffsetType).unwrap();
    storage.delete_vector(4 as PointOffsetType).unwrap();
    assert_eq!(
        storage.deleted_vector_count(),
        5,
        "all vectors must be deleted"
    );

    let vector: Vec<Vec<f32>> = vec![vec![1.0; vector_dim]];
    let query = QueryVector::Nearest(vector.try_into().unwrap());
    let scorer =
        FilteredScorer::new_for_test(query, storage, borrowed_id_tracker.deleted_point_bitslice());
    let closest = scorer.peek_top_all(5, &DEFAULT_STOPPED).unwrap();
    assert!(closest.is_empty(), "must have no results, all deleted");
}

fn do_test_update_from_delete_points(
    vector_dim: usize,
    vec_count: usize,
    storage: &mut VectorStorageEnum,
) {
    let points = multi_points_fixtures(vec_count, vector_dim);

    let delete_mask = [false, false, true, true, false];

    let id_tracker: Arc<AtomicRefCell<IdTrackerSS>> =
        Arc::new(AtomicRefCell::new(FixtureIdTracker::new(points.len())));
    let borrowed_id_tracker = id_tracker.borrow_mut();

    let hw_counter = HardwareCounterCell::new();

    {
        let mut storage2 = new_volatile_multi_dense_vector_storage(
            vector_dim,
            Distance::Dot,
            MultiVectorConfig::default(),
        );
        {
            points.iter().enumerate().for_each(|(i, vec)| {
                storage2
                    .insert_vector(i as PointOffsetType, vec.into(), &hw_counter)
                    .unwrap();
                if delete_mask[i] {
                    storage2.delete_vector(i as PointOffsetType).unwrap();
                }
            });
        }
        let mut iter = (0..points.len()).map(|i| {
            let i = i as PointOffsetType;
            let vec = storage2.get_vector(i);
            let deleted = storage2.is_deleted_vector(i);
            (vec, deleted)
        });
        storage.update_from(&mut iter, &Default::default()).unwrap();
    }

    assert_eq!(
        storage.deleted_vector_count(),
        2,
        "2 vectors must be deleted from other storage"
    );

    let vector: Vec<Vec<f32>> = vec![vec![1.0; vector_dim]];

    let query = QueryVector::Nearest(vector.try_into().unwrap());

    let scorer =
        FilteredScorer::new_for_test(query, storage, borrowed_id_tracker.deleted_point_bitslice());
    let closest = scorer
        .peek_top_iter(&mut [0, 1, 2, 3, 4].iter().cloned(), 5, &DEFAULT_STOPPED)
        .unwrap();
    drop(scorer);
    assert_eq!(closest.len(), 3, "must have 3 vectors, 2 are deleted");
    assert_eq!(closest[0].idx, 4);
    assert_eq!(closest[1].idx, 1);
    assert_eq!(closest[2].idx, 0);

    // Delete all
    storage.delete_vector(0 as PointOffsetType).unwrap();
    storage.delete_vector(1 as PointOffsetType).unwrap();
    storage.delete_vector(4 as PointOffsetType).unwrap();
    assert_eq!(
        storage.deleted_vector_count(),
        5,
        "all vectors must be deleted"
    );
}

fn create_vector_storage(
    storage_type: MultiDenseStorageType,
    vec_dim: usize,
    path: &Path,
) -> VectorStorageEnum {
    match storage_type {
        #[cfg(feature = "rocksdb")]
        MultiDenseStorageType::RocksDbFloat => {
            use crate::common::rocksdb_wrapper::{DB_VECTOR_CF, open_db};
            use crate::vector_storage::multi_dense::simple_multi_dense_vector_storage::open_simple_multi_dense_vector_storage_full;

            let db = open_db(path, &[DB_VECTOR_CF]).unwrap();
            open_simple_multi_dense_vector_storage_full(
                db,
                DB_VECTOR_CF,
                vec_dim,
                Distance::Dot,
                MultiVectorConfig::default(),
                &Default::default(),
            )
            .unwrap()
        }
        MultiDenseStorageType::AppendableMmapFloat => {
            open_appendable_memmap_multi_vector_storage_full(
                path,
                vec_dim,
                Distance::Dot,
                MultiVectorConfig::default(),
            )
            .unwrap()
        }
    }
}

#[rstest]
#[cfg_attr(feature = "rocksdb", case(MultiDenseStorageType::RocksDbFloat))]
#[case(MultiDenseStorageType::AppendableMmapFloat)]
fn test_delete_points_in_multi_dense_vector_storage(#[case] storage_type: MultiDenseStorageType) {
    let vec_dim = 1024;
    let vec_count = 5;
    let dir = Builder::new().prefix("storage_dir").tempdir().unwrap();
    let total_vector_count = {
        let mut storage = create_vector_storage(storage_type, vec_dim, dir.path());
        do_test_delete_points(vec_dim, vec_count, &mut storage);
        let count = storage.total_vector_count();
        storage.flusher()().unwrap();
        count
    };
    let storage = create_vector_storage(storage_type, vec_dim, dir.path());
    assert_eq!(
        storage.total_vector_count(),
        total_vector_count,
        "total vector count must be the same"
    );
    // retrieve all vectors from storage
    for id in 0..total_vector_count {
        assert!(storage.get_vector_opt(id as PointOffsetType).is_some());
    }
}

#[rstest]
#[cfg_attr(feature = "rocksdb", case(MultiDenseStorageType::RocksDbFloat))]
#[case(MultiDenseStorageType::AppendableMmapFloat)]
fn test_update_from_delete_points_multi_dense_vector_storage(
    #[case] storage_type: MultiDenseStorageType,
) {
    let vec_dim = 1024;
    let vec_count = 5;
    let dir = Builder::new().prefix("storage_dir").tempdir().unwrap();
    let total_vector_count = {
        let mut storage = create_vector_storage(storage_type, vec_dim, dir.path());
        do_test_update_from_delete_points(vec_dim, vec_count, &mut storage);
        let count = storage.total_vector_count();
        storage.flusher()().unwrap();
        count
    };
    let storage = create_vector_storage(storage_type, vec_dim, dir.path());
    assert_eq!(
        storage.total_vector_count(),
        total_vector_count,
        "total vector count must be the same"
    );
    // retrieve all vectors from storage
    for id in 0..total_vector_count {
        assert!(storage.get_vector_opt(id as PointOffsetType).is_some());
    }
}

#[rstest]
#[cfg_attr(feature = "rocksdb", case(MultiDenseStorageType::RocksDbFloat))]
#[case(MultiDenseStorageType::AppendableMmapFloat)]
fn test_large_multi_dense_vector_storage(#[case] storage_type: MultiDenseStorageType) {
    assert!(MAX_MULTIVECTOR_FLATTENED_LEN * std::mem::size_of::<VectorElementType>() < CHUNK_SIZE);

    let vec_dim = 100_000;
    let vec_count = 100;
    let dir = Builder::new().prefix("storage_dir").tempdir().unwrap();
    let mut storage = create_vector_storage(storage_type, vec_dim, dir.path());

    let vectors = vec![vec![0.0; vec_dim]; vec_count];
    let multivec = MultiDenseVectorInternal::try_from(vectors).unwrap();

    let hw_counter = HardwareCounterCell::new();
    let result = storage.insert_vector(0, VectorRef::from(&multivec), &hw_counter);
    match result {
        Ok(_) => {
            panic!("Inserting vector should fail");
        }
        Err(e) => {
            assert!(e.to_string().contains("too large"));
        }
    }
}

#[test]
fn test_delete_points_in_volatile_multi_dense_vector_storage() {
    let vec_dim = 1024;
    let vec_count = 5;
    let mut storage = new_volatile_multi_dense_vector_storage(
        vec_dim,
        Distance::Dot,
        MultiVectorConfig::default(),
    );
    do_test_delete_points(vec_dim, vec_count, &mut storage);

    // retrieve all vectors from storage
    for id in 0..storage.total_vector_count() {
        assert!(storage.get_vector_opt(id as PointOffsetType).is_some());
    }
}

#[test]
fn test_update_from_delete_points_volatile_multi_dense_vector_storage() {
    let vec_dim = 1024;
    let vec_count = 5;
    let mut storage = new_volatile_multi_dense_vector_storage(
        vec_dim,
        Distance::Dot,
        MultiVectorConfig::default(),
    );
    do_test_update_from_delete_points(vec_dim, vec_count, &mut storage);

    // retrieve all vectors from storage
    for id in 0..storage.total_vector_count() {
        assert!(storage.get_vector_opt(id as PointOffsetType).is_some());
    }
}

#[test]
fn test_large_volatile_multi_dense_vector_storage() {
    assert!(MAX_MULTIVECTOR_FLATTENED_LEN * std::mem::size_of::<VectorElementType>() < CHUNK_SIZE);

    let vec_dim = 100_000;
    let vec_count = 100;
    let mut storage = new_volatile_multi_dense_vector_storage(
        vec_dim,
        Distance::Dot,
        MultiVectorConfig::default(),
    );

    let vectors = vec![vec![0.0; vec_dim]; vec_count];
    let multivec = MultiDenseVectorInternal::try_from(vectors).unwrap();

    let hw_counter = HardwareCounterCell::new();
    let result = storage.insert_vector(0, VectorRef::from(&multivec), &hw_counter);
    match result {
        Ok(_) => {
            panic!("Inserting vector should fail");
        }
        Err(e) => {
            assert!(e.to_string().contains("too large"));
        }
    }
}
