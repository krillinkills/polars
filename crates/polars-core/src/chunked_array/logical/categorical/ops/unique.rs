use polars_compute::unique::{
    DictionaryRangedUniqueState, PrimitiveRangedUniqueState, RangedUniqueKernel,
};

use super::*;

impl CategoricalChunked {
    pub fn unique(&self) -> PolarsResult<Self> {
        let cat_map = self.get_rev_map();
        if self.is_empty() {
            // SAFETY: rev map is valid.
            unsafe {
                return Ok(CategoricalChunked::from_cats_and_rev_map_unchecked(
                    UInt32Chunked::full_null(self.name().clone(), 0),
                    cat_map.clone(),
                    self.is_enum(),
                    self.get_ordering(),
                ));
            }
        };

        if self._can_fast_unique() {
            let ca = match &**cat_map {
                RevMapping::Local(a, _) => UInt32Chunked::from_iter_values(
                    self.physical().name().clone(),
                    0..(a.len() as u32),
                ),
                RevMapping::Global(map, _, _) => UInt32Chunked::from_iter_values(
                    self.physical().name().clone(),
                    map.keys().copied(),
                ),
            };
            // SAFETY:
            // we only removed some indexes so we are still in bounds
            unsafe {
                let mut out = CategoricalChunked::from_cats_and_rev_map_unchecked(
                    ca,
                    cat_map.clone(),
                    self.is_enum(),
                    self.get_ordering(),
                );
                out.set_fast_unique(true);
                Ok(out)
            }
        } else {
            let has_nulls = (self.null_count() > 0) as u32;
            let mut state = match cat_map.as_ref() {
                RevMapping::Global(map, values, _) => {
                    if self.is_enum() {
                        PrimitiveRangedUniqueState::new(0, values.len() as u32 + has_nulls)
                    } else {
                        let mut min = u32::MAX;
                        let mut max = 0u32;

                        for &v in map.keys() {
                            min = min.min(v);
                            max = max.max(v);
                        }

                        PrimitiveRangedUniqueState::new(min, max + has_nulls)
                    }
                },
                RevMapping::Local(values, _) => {
                    PrimitiveRangedUniqueState::new(0, values.len() as u32 + has_nulls)
                },
            };

            for chunk in self.physical().downcast_iter() {
                state.append(chunk);
            }
            let unique = state.finalize_unique();
            let ca = unsafe {
                UInt32Chunked::from_chunks_and_dtype_unchecked(
                    self.physical().name().clone(),
                    vec![unique.to_boxed()],
                    DataType::UInt32,
                )
            };
            // SAFETY:
            // we only removed some indexes so we are still in bounds
            unsafe {
                Ok(CategoricalChunked::from_cats_and_rev_map_unchecked(
                    ca,
                    cat_map.clone(),
                    self.is_enum(),
                    self.get_ordering(),
                ))
            }
        }
    }

    pub fn n_unique(&self) -> PolarsResult<usize> {
        if self._can_fast_unique() {
            Ok(self.get_rev_map().len())
        } else {
            let cat_map = self.get_rev_map();
            let mut state = DictionaryRangedUniqueState::new(cat_map.get_categories().to_boxed());
            for chunk in self.physical().downcast_iter() {
                state.key_state().append(chunk);
            }
            Ok(state.finalize_n_unique())
        }
    }

    pub fn value_counts(&self) -> PolarsResult<DataFrame> {
        let groups = self.physical().group_tuples(true, false).unwrap();
        let physical_values = unsafe {
            self.physical()
                .clone()
                .into_series()
                .agg_first(&groups)
                .u32()
                .unwrap()
                .clone()
        };

        let mut values = self.clone();
        *values.physical_mut() = physical_values;

        let mut counts = groups.group_count();
        counts.rename(PlSmallStr::from_static("counts"));
        let height = counts.len();
        let cols = vec![values.into_series().into(), counts.into_series().into()];
        let df = unsafe { DataFrame::new_no_checks(height, cols) };
        df.sort(
            ["counts"],
            SortMultipleOptions::default().with_order_descending(true),
        )
    }
}
