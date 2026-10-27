use std::collections::BTreeMap;
use std::ffi::c_void;
use std::mem;

use windows::Win32::System::Diagnostics::Debug::ReadProcessMemory;
use windows::Win32::System::Threading::GetCurrentProcess;

pub(super) fn bytes(address: usize, size: usize) -> Option<Vec<u8>> {
    if address == 0 || size > 64 * 1024 * 1024 {
        return None;
    }
    address.checked_add(size)?;
    let mut data = vec![0; size];
    let mut read = 0;
    unsafe {
        ReadProcessMemory(
            GetCurrentProcess(),
            address as *const c_void,
            data.as_mut_ptr().cast(),
            size,
            Some(&mut read),
        )
        .ok()?;
    }
    (read == size).then_some(data)
}

pub(super) fn value<T: Copy>(address: usize) -> Option<T> {
    let data = bytes(address, mem::size_of::<T>())?;
    Some(unsafe { data.as_ptr().cast::<T>().read_unaligned() })
}

pub(super) fn at<T: Copy>(data: &[u8], offset: usize) -> Option<T> {
    let data = data.get(offset..offset.checked_add(mem::size_of::<T>())?)?;
    Some(unsafe { data.as_ptr().cast::<T>().read_unaligned() })
}

fn param_name(address: usize) -> Option<String> {
    // DLWString's capacity, rather than its length, selects the inline buffer.
    let name = bytes(address.checked_add(0x18)?, 0x20)?;
    let length: usize = at(&name, 0x10)?;
    let capacity: usize = at(&name, 0x18)?;
    if length == 0 || length > 128 || capacity < length {
        return None;
    }
    let data = if capacity <= 7 {
        name.get(..length * 2)?.to_vec()
    } else {
        bytes(at(&name, 0)?, length * 2)?
    };
    let data: Vec<_> =
        data.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    String::from_utf16(&data).ok()
}

#[derive(Debug)]
pub(super) struct ParamTables(BTreeMap<String, ParamTable>);

impl ParamTables {
    pub(super) fn read(regulation_manager: usize, wanted: &[&str]) -> Option<Self> {
        let manager: usize = value(regulation_manager)?;
        if manager == 0 {
            return None;
        }
        // This is the same CSRegulationManager chain used by crate::params.
        let master = bytes(manager.checked_add(0x18)?, 0x10)?;
        let start: usize = at(&master, 0)?;
        let end: usize = at(&master, 8)?;
        let length = end.checked_sub(start)?;
        if length == 0 || length % 8 != 0 || length / 8 > 4096 {
            return None;
        }
        let entries = bytes(start, length)?;
        let mut tables = BTreeMap::new();
        for entry in entries.as_chunks::<8>().0 {
            let address = at::<usize>(entry, 0)?;
            if address == 0 {
                continue;
            }
            if let Some((name, table)) = param_name(address).and_then(|name| {
                if !wanted.contains(&name.as_str()) {
                    return None;
                }
                let header: usize = value(address.checked_add(0x80)?)?;
                if header == 0 {
                    return None;
                }
                let table: usize = value(header.checked_add(0x80)?)?;
                Some((name, ParamTable::read(table)?))
            }) {
                tables.insert(name, table);
            }
        }
        (!tables.is_empty()).then_some(Self(tables))
    }

    pub(super) fn get(&self, name: &str) -> Option<&ParamTable> {
        self.0.get(name)
    }
}

#[derive(Debug)]
pub(super) struct ParamTable {
    rows: Vec<(u32, usize)>,
}

impl ParamTable {
    fn read(address: usize) -> Option<Self> {
        let count: u16 = value(address.checked_add(0xa)?)?;
        let entries = bytes(address.checked_add(0x40)?, usize::from(count) * 0x18)?;
        let minimum_offset = 0x40 + entries.len();
        let mut rows = Vec::with_capacity(count.into());
        for entry in entries.as_chunks::<0x18>().0 {
            let id: u64 = at(entry, 0)?;
            let offset: usize = at(entry, 8)?;
            // Reject sentinels and corrupt offsets before reading any row data.
            if let Ok(id) = u32::try_from(id) {
                if offset >= minimum_offset && offset < 64 * 1024 * 1024 {
                    rows.push((id, address.checked_add(offset)?));
                }
            }
        }
        rows.sort_unstable_by_key(|&(id, _)| id);
        Some(Self { rows })
    }

    pub(super) fn ids(&self) -> impl Iterator<Item = u32> + '_ {
        self.rows.iter().map(|&(id, _)| id)
    }

    pub(super) fn rows(&self, size: usize) -> impl Iterator<Item = (u32, Vec<u8>)> + '_ {
        self.rows.iter().filter_map(move |&(id, address)| Some((id, bytes(address, size)?)))
    }

    pub(super) fn row(&self, id: u32, size: usize) -> Option<Vec<u8>> {
        let index = self.rows.binary_search_by_key(&id, |&(row_id, _)| row_id).ok()?;
        bytes(self.rows[index].1, size)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_memory_and_truncated_fields_are_rejected() {
        assert!(bytes(0, 8).is_none());
        assert!(bytes(usize::MAX - 1, 8).is_none());
        assert!(at::<u32>(&[1, 2, 3], 0).is_none());
        assert!(at::<u32>(&[0; 8], usize::MAX).is_none());
    }

    #[test]
    fn param_rows_are_read_from_the_table_not_row_names() {
        let mut table = [0u8; 0x78];
        table[0xa..0xc].copy_from_slice(&2u16.to_le_bytes());
        table[0x40..0x48].copy_from_slice(&99u64.to_le_bytes());
        table[0x48..0x50].copy_from_slice(&0x74u64.to_le_bytes());
        table[0x58..0x60].copy_from_slice(&42u64.to_le_bytes());
        table[0x60..0x68].copy_from_slice(&0x70u64.to_le_bytes());
        table[0x70..0x74].copy_from_slice(&123u32.to_le_bytes());
        table[0x74..0x78].copy_from_slice(&456u32.to_le_bytes());
        let table = ParamTable::read(table.as_ptr() as usize).unwrap();
        assert_eq!(table.ids().collect::<Vec<_>>(), [42, 99]);
        assert_eq!(at::<u32>(&table.row(42, 4).unwrap(), 0), Some(123));
        assert_eq!(at::<u32>(&table.row(99, 4).unwrap(), 0), Some(456));
        assert!(table.row(43, 4).is_none());
    }
}
