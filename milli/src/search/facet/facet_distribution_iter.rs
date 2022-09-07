use std::ops::ControlFlow;

use heed::Result;
use roaring::RoaringBitmap;

use super::{get_first_facet_value, get_highest_level};
use crate::heed_codec::facet::{
    ByteSliceRef, FacetGroupKey, FacetGroupKeyCodec, FacetGroupValueCodec,
};
use crate::DocumentId;

pub fn iterate_over_facet_distribution<'t, CB>(
    rtxn: &'t heed::RoTxn<'t>,
    db: heed::Database<FacetGroupKeyCodec<ByteSliceRef>, FacetGroupValueCodec>,
    field_id: u16,
    candidates: &RoaringBitmap,
    callback: CB,
) -> Result<()>
where
    CB: FnMut(&'t [u8], u64, DocumentId) -> Result<ControlFlow<()>>,
{
    let mut fd = FacetDistribution { rtxn, db, field_id, callback };
    let highest_level =
        get_highest_level(rtxn, db.remap_key_type::<FacetGroupKeyCodec<ByteSliceRef>>(), field_id)?;

    if let Some(first_bound) = get_first_facet_value::<ByteSliceRef>(rtxn, db, field_id)? {
        fd.iterate(candidates, highest_level, first_bound, usize::MAX)?;
        return Ok(());
    } else {
        return Ok(());
    }
}

struct FacetDistribution<'t, CB>
where
    CB: FnMut(&'t [u8], u64, DocumentId) -> Result<ControlFlow<()>>,
{
    rtxn: &'t heed::RoTxn<'t>,
    db: heed::Database<FacetGroupKeyCodec<ByteSliceRef>, FacetGroupValueCodec>,
    field_id: u16,
    callback: CB,
}

impl<'t, CB> FacetDistribution<'t, CB>
where
    CB: FnMut(&'t [u8], u64, DocumentId) -> Result<ControlFlow<()>>,
{
    fn iterate_level_0(
        &mut self,
        candidates: &RoaringBitmap,
        starting_bound: &'t [u8],
        group_size: usize,
    ) -> Result<ControlFlow<()>> {
        let starting_key =
            FacetGroupKey { field_id: self.field_id, level: 0, left_bound: starting_bound };
        let iter = self.db.range(self.rtxn, &(starting_key..))?.take(group_size);
        for el in iter {
            let (key, value) = el?;
            // The range is unbounded on the right and the group size for the highest level is MAX,
            // so we need to check that we are not iterating over the next field id
            if key.field_id != self.field_id {
                return Ok(ControlFlow::Break(()));
            }
            let docids_in_common = value.bitmap.intersection_len(candidates);
            if docids_in_common > 0 {
                let any_docid = value.bitmap.iter().next().unwrap();
                match (self.callback)(key.left_bound, docids_in_common, any_docid)? {
                    ControlFlow::Continue(_) => {}
                    ControlFlow::Break(_) => return Ok(ControlFlow::Break(())),
                }
            }
        }
        return Ok(ControlFlow::Continue(()));
    }
    fn iterate(
        &mut self,
        candidates: &RoaringBitmap,
        level: u8,
        starting_bound: &'t [u8],
        group_size: usize,
    ) -> Result<ControlFlow<()>> {
        if level == 0 {
            return self.iterate_level_0(candidates, starting_bound, group_size);
        }
        let starting_key =
            FacetGroupKey { field_id: self.field_id, level, left_bound: starting_bound };
        let iter = self.db.range(&self.rtxn, &(&starting_key..)).unwrap().take(group_size);

        for el in iter {
            let (key, value) = el.unwrap();
            // The range is unbounded on the right and the group size for the highest level is MAX,
            // so we need to check that we are not iterating over the next field id
            if key.field_id != self.field_id {
                return Ok(ControlFlow::Break(()));
            }
            let docids_in_common = value.bitmap & candidates;
            if docids_in_common.len() > 0 {
                let cf = self.iterate(
                    &docids_in_common,
                    level - 1,
                    key.left_bound,
                    value.size as usize,
                )?;
                match cf {
                    ControlFlow::Continue(_) => {}
                    ControlFlow::Break(_) => return Ok(ControlFlow::Break(())),
                }
            }
        }

        return Ok(ControlFlow::Continue(()));
    }
}

#[cfg(test)]
mod tests {
    use std::ops::ControlFlow;

    use heed::BytesDecode;
    use roaring::RoaringBitmap;

    use super::iterate_over_facet_distribution;
    use crate::heed_codec::facet::OrderedF64Codec;
    use crate::milli_snap;
    use crate::search::facet::tests::{get_random_looking_index, get_simple_index};

    #[test]
    fn filter_distribution_all() {
        let indexes = [get_simple_index(), get_random_looking_index()];
        for (i, index) in indexes.iter().enumerate() {
            let txn = index.env.read_txn().unwrap();
            let candidates = (0..=255).into_iter().collect::<RoaringBitmap>();
            let mut results = String::new();
            iterate_over_facet_distribution(
                &txn,
                index.content,
                0,
                &candidates,
                |facet, count, _| {
                    let facet = OrderedF64Codec::bytes_decode(facet).unwrap();
                    results.push_str(&format!("{facet}: {count}\n"));
                    Ok(ControlFlow::Continue(()))
                },
            )
            .unwrap();
            milli_snap!(results, i);

            txn.commit().unwrap();
        }
    }
    #[test]
    fn filter_distribution_all_stop_early() {
        let indexes = [get_simple_index(), get_random_looking_index()];
        for (i, index) in indexes.iter().enumerate() {
            let txn = index.env.read_txn().unwrap();
            let candidates = (0..=255).into_iter().collect::<RoaringBitmap>();
            let mut results = String::new();
            let mut nbr_facets = 0;
            iterate_over_facet_distribution(
                &txn,
                index.content,
                0,
                &candidates,
                |facet, count, _| {
                    let facet = OrderedF64Codec::bytes_decode(facet).unwrap();
                    if nbr_facets == 100 {
                        return Ok(ControlFlow::Break(()));
                    } else {
                        nbr_facets += 1;
                        results.push_str(&format!("{facet}: {count}\n"));

                        Ok(ControlFlow::Continue(()))
                    }
                },
            )
            .unwrap();
            milli_snap!(results, i);

            txn.commit().unwrap();
        }
    }
}
