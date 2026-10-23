use sha2::{Digest, Sha256};

pub const BUNDLE_RELATIVE: &str = "StarTrekOutpostsUnknown_Data/StreamingAssets/aa/StandaloneWindows64/defaultlocalgroup_scenes_all_0db1db0c047f9687df555fddfb068366.bundle";
pub const ORIGINAL_HASH: &str = "223c0f0bb3cda089e35954cd5f284acef501cca60d5ff365e9288453eab6c070";
const ORIGINAL_SIZE: usize = 148_249_826;
const INFO_START: usize = 64;
const INFO_COMPRESSED: usize = 13_274;
const INFO_SIZE: usize = 33_522;
const BLOCK_START: usize = 4_790_123;
const BLOCK_COMPRESSED: usize = 33_261;
const BLOCK_SIZE: usize = 131_072;
const FIELD_OFFSET: usize = 98_832;
const BLOCK_INDEX: usize = 53;

fn valid_limit(limit: f32) -> bool {
    limit.is_finite() && (300.0..=1000.0).contains(&limit)
}

fn patch_block(compressed: &[u8], limit: f32) -> Result<Vec<u8>, String> {
    let mut block = lz4_flex::block::decompress(compressed, BLOCK_SIZE)
        .map_err(|e| format!("Kamerablock unlesbar: {e}"))?;
    if block.len() != BLOCK_SIZE || block[FIELD_OFFSET..FIELD_OFFSET + 4] != 300.0f32.to_le_bytes()
    {
        return Err("Kamerablock enthält nicht den erwarteten Originalwert.".into());
    }
    block[FIELD_OFFSET..FIELD_OFFSET + 4].copy_from_slice(&limit.to_le_bytes());
    Ok(lz4_flex::block::compress(&block))
}

/// Patches the camera's serialized maximum; all other archive blocks stay byte-identical.
pub fn patch(original: &[u8], limit: f32) -> Result<Vec<u8>, String> {
    if !valid_limit(limit) {
        return Err("Permanenter Zoom muss zwischen 300 und 1000 liegen.".into());
    }
    // shortcut: offsets belong to this exact bundle; reanalyze before supporting another build.
    if original.len() != ORIGINAL_SIZE
        || Sha256::digest(original)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
            != ORIGINAL_HASH
    {
        return Err(
            "Szenen-Bundle unbekannt oder verändert; permanente Änderung abgelehnt.".into(),
        );
    }
    if limit == 300.0 {
        return Ok(original.to_vec());
    }
    let mut info = lz4_flex::block::decompress(
        &original[INFO_START..INFO_START + INFO_COMPRESSED],
        INFO_SIZE,
    )
    .map_err(|e| format!("Bundle-Verzeichnis unlesbar: {e}"))?;
    if info.len() != INFO_SIZE {
        return Err("Bundle-Verzeichnis hat unerwartete Länge.".into());
    }
    let block = patch_block(
        &original[BLOCK_START..BLOCK_START + BLOCK_COMPRESSED],
        limit,
    )?;
    let size_field = 20 + 10 * BLOCK_INDEX + 4;
    info[size_field..size_field + 4].copy_from_slice(&(block.len() as u32).to_be_bytes());
    let packed_info = lz4_flex::block::compress(&info);
    let old_data_start = (INFO_START + INFO_COMPRESSED).next_multiple_of(16);
    let new_data_start = (INFO_START + packed_info.len()).next_multiple_of(16);
    let mut output = Vec::with_capacity(original.len() + packed_info.len() + block.len());
    output.extend_from_slice(&original[..INFO_START]);
    output.extend_from_slice(&packed_info);
    output.resize(new_data_start, 0);
    output.extend_from_slice(&original[old_data_start..BLOCK_START]);
    output.extend_from_slice(&block);
    output.extend_from_slice(&original[BLOCK_START + BLOCK_COMPRESSED..]);
    let output_size = output.len() as u64;
    output[30..38].copy_from_slice(&output_size.to_be_bytes());
    output[38..42].copy_from_slice(&(packed_info.len() as u32).to_be_bytes());
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limit_rejects_nonfinite_and_out_of_range_values() {
        for limit in [f32::NAN, f32::INFINITY, 299.0, 1001.0] {
            assert!(!valid_limit(limit));
            assert!(patch(&[], limit).is_err());
        }
        for limit in [300.0, 750.0, 1000.0] {
            assert!(valid_limit(limit));
        }
        assert!(patch(&[], 750.0).is_err());
    }

    #[test]
    fn compressed_patch_changes_only_camera_float() {
        let mut original: Vec<u8> = (0..BLOCK_SIZE).map(|i| (i % 251) as u8).collect();
        original[FIELD_OFFSET..FIELD_OFFSET + 4].copy_from_slice(&300.0f32.to_le_bytes());
        let packed = lz4_flex::block::compress(&original);
        let result = patch_block(&packed, 1000.0).unwrap();
        let result = lz4_flex::block::decompress(&result, BLOCK_SIZE).unwrap();
        original[FIELD_OFFSET..FIELD_OFFSET + 4].copy_from_slice(&1000.0f32.to_le_bytes());
        assert_eq!(result, original);
        assert!(patch_block(&[0], 1000.0).is_err());
    }

    #[test]
    #[ignore = "requires the exact installed game; never writes game files"]
    fn installed_bundle_round_trip() {
        let original = std::fs::read(
            std::path::PathBuf::from(
                std::env::var_os("OUTPOSTS_GAME_DIR").expect("set OUTPOSTS_GAME_DIR"),
            )
            .join(BUNDLE_RELATIVE),
        )
        .unwrap();
        let patched = patch(&original, 1000.0).unwrap();
        let info_size = u32::from_be_bytes(patched[38..42].try_into().unwrap()) as usize;
        let info = lz4_flex::block::decompress(&patched[64..64 + info_size], INFO_SIZE).unwrap();
        let new_block_start = (64 + info_size).next_multiple_of(16) + BLOCK_START
            - (64 + INFO_COMPRESSED).next_multiple_of(16);
        let field = 20 + BLOCK_INDEX * 10 + 4;
        let block_size = u32::from_be_bytes(info[field..field + 4].try_into().unwrap()) as usize;
        let block = lz4_flex::block::decompress(
            &patched[new_block_start..new_block_start + block_size],
            BLOCK_SIZE,
        )
        .unwrap();
        let mut expected = lz4_flex::block::decompress(
            &original[BLOCK_START..BLOCK_START + BLOCK_COMPRESSED],
            BLOCK_SIZE,
        )
        .unwrap();
        expected[FIELD_OFFSET..FIELD_OFFSET + 4].copy_from_slice(&1000.0f32.to_le_bytes());
        assert_eq!(block, expected);
        assert_eq!(patch(&original, 300.0).unwrap(), original);
        std::fs::create_dir_all("analysis").unwrap();
        std::fs::write("analysis/scenes-rust-zoom1000.bundle", patched).unwrap();
    }
}
