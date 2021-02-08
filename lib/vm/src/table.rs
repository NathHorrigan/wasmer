// This file contains code from external sources.
// Attributions: https://github.com/wasmerio/wasmer/blob/master/ATTRIBUTIONS.md

//! Memory management for tables.
//!
//! `Table` is to WebAssembly tables what `LinearMemory` is to WebAssembly linear memories.

use crate::extern_ref::VMExternRef;
use crate::func_data_registry::VMFuncRef;
use crate::trap::{Trap, TrapCode};
use crate::vmcontext::VMTableDefinition;
use serde::{Deserialize, Serialize};
use std::borrow::{Borrow, BorrowMut};
use std::cell::UnsafeCell;
use std::convert::TryFrom;
use std::fmt;
use std::ptr::NonNull;
use std::sync::Mutex;
use wasmer_types::{TableType, Type as ValType};

/// Implementation styles for WebAssembly tables.
#[derive(Debug, Clone, Hash, Serialize, Deserialize)]
pub enum TableStyle {
    /// Signatures are stored in the table and checked in the caller.
    CallerChecksSignature,
}

/// Trait for implementing the interface of a Wasm table.
pub trait Table: fmt::Debug + Send + Sync {
    /// Returns the style for this Table.
    fn style(&self) -> &TableStyle;

    /// Returns the type for this Table.
    fn ty(&self) -> &TableType;

    /// Returns the number of allocated elements.
    fn size(&self) -> u32;

    /// Grow table by the specified amount of elements.
    ///
    /// Returns `None` if table can't be grown by the specified amount
    /// of elements, otherwise returns the previous size of the table.
    fn grow(&self, delta: u32) -> Option<u32>;

    /// Get reference to the specified element.
    ///
    /// Returns `None` if the index is out of bounds.
    fn get(&self, index: u32) -> Result<TableReference, Trap>;

    /// Set reference to the specified element.
    ///
    /// # Errors
    ///
    /// Returns an error if the index is out of bounds.
    fn set(&self, index: u32, reference: TableReference) -> Result<(), Trap>;

    /// Return a `VMTableDefinition` for exposing the table to compiled wasm code.
    fn vmtable(&self) -> NonNull<VMTableDefinition>;

    /// Copy `len` elements from `src_table[src_index..]` into `dst_table[dst_index..]`.
    ///
    /// # Errors
    ///
    /// Returns an error if the range is out of bounds of either the source or
    /// destination tables.
    fn copy(
        &self,
        src_table: &dyn Table,
        dst_index: u32,
        src_index: u32,
        len: u32,
    ) -> Result<(), Trap> {
        // https://webassembly.github.io/bulk-memory-operations/core/exec/instructions.html#exec-table-copy

        if src_index
            .checked_add(len)
            .map_or(true, |n| n > src_table.size())
        {
            return Err(Trap::new_from_runtime(TrapCode::TableAccessOutOfBounds));
        }

        if dst_index.checked_add(len).map_or(true, |m| m > self.size()) {
            return Err(Trap::new_from_runtime(TrapCode::TableSetterOutOfBounds));
        }

        let srcs = src_index..src_index + len;
        let dsts = dst_index..dst_index + len;

        // Note on the unwraps: the bounds check above means that these will
        // never panic.
        //
        // TODO: investigate replacing this get/set loop with a `memcpy`.
        if dst_index <= src_index {
            for (s, d) in (srcs).zip(dsts) {
                self.set(d, src_table.get(s).unwrap())?;
            }
        } else {
            for (s, d) in srcs.rev().zip(dsts.rev()) {
                self.set(d, src_table.get(s).unwrap())?;
            }
        }

        Ok(())
    }
}

/// A reference stored in a table. Can be either an externref or a funcref.
#[derive(Debug, Clone)]
pub enum TableReference {
    // TODO: implement extern refs
    /// Opaque pointer to arbitrary host data.
    ExternRef(VMExternRef),
    /// Pointer to function: contains enough information to call it.
    FuncRef(VMFuncRef),
}

impl From<TableReference> for TableElement {
    fn from(other: TableReference) -> Self {
        match other {
            TableReference::ExternRef(extern_ref) => Self { extern_ref },
            TableReference::FuncRef(func_ref) => Self { func_ref },
        }
    }
}

#[derive(Clone, Copy)]
union TableElement {
    extern_ref: VMExternRef,
    func_ref: VMFuncRef,
}

impl fmt::Debug for TableElement {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("TableElement").finish()
    }
}

impl Default for TableElement {
    fn default() -> Self {
        Self {
            func_ref: VMFuncRef::null(),
        }
    }
}

impl Default for TableReference {
    fn default() -> Self {
        Self::FuncRef(VMFuncRef::null())
    }
}

/// A table instance.
#[derive(Debug)]
pub struct LinearTable {
    // TODO: we can remove the mutex by using atomic swaps and preallocating the max table size
    vec: Mutex<Vec<TableElement>>,
    maximum: Option<u32>,
    /// The WebAssembly table description.
    table: TableType,
    /// Our chosen implementation style.
    style: TableStyle,
    vm_table_definition: VMTableDefinitionOwnership,
}

/// A type to help manage who is responsible for the backing table of the
/// `VMTableDefinition`.
#[derive(Debug)]
enum VMTableDefinitionOwnership {
    /// The `VMTableDefinition` is owned by the `Instance` and we should use
    /// its table. This is how a local table that's exported should be stored.
    VMOwned(NonNull<VMTableDefinition>),
    /// The `VMTableDefinition` is owned by the host and we should manage its
    /// table. This is how an imported table that doesn't come from another
    /// Wasm module should be stored.
    HostOwned(Box<UnsafeCell<VMTableDefinition>>),
}

/// This is correct because there is no thread-specific data tied to this type.
unsafe impl Send for LinearTable {}
/// This is correct because all internal mutability is protected by a mutex.
unsafe impl Sync for LinearTable {}

impl LinearTable {
    /// Create a new linear table instance with specified minimum and maximum number of elements.
    ///
    /// This creates a `LinearTable` with metadata owned by a VM, pointed to by
    /// `vm_table_location`: this can be used to create a local table.
    pub fn new(table: &TableType, style: &TableStyle) -> Result<Self, String> {
        unsafe { Self::new_inner(table, style, None) }
    }

    /// Create a new linear table instance with specified minimum and maximum number of elements.
    ///
    /// This creates a `LinearTable` with metadata owned by a VM, pointed to by
    /// `vm_table_location`: this can be used to create a local table.
    ///
    /// # Safety
    /// - `vm_table_location` must point to a valid location in VM memory.
    pub unsafe fn from_definition(
        table: &TableType,
        style: &TableStyle,
        vm_table_location: NonNull<VMTableDefinition>,
    ) -> Result<Self, String> {
        Self::new_inner(table, style, Some(vm_table_location))
    }

    /// Create a new `LinearTable` with either self-owned or VM owned metadata.
    unsafe fn new_inner(
        table: &TableType,
        style: &TableStyle,
        vm_table_location: Option<NonNull<VMTableDefinition>>,
    ) -> Result<Self, String> {
        match table.ty {
            ValType::FuncRef | ValType::ExternRef => (),
            ty => {
                return Err(format!(
                    "tables of types other than funcref or externref ({})",
                    ty
                ))
            }
        };
        if let Some(max) = table.maximum {
            if max < table.minimum {
                return Err(format!(
                    "Table minimum ({}) is larger than maximum ({})!",
                    table.minimum, max
                ));
            }
        }
        let table_minimum = usize::try_from(table.minimum)
            .map_err(|_| "Table minimum is bigger than usize".to_string())?;
        let mut vec = vec![TableElement::default(); table_minimum];
        let base = vec.as_mut_ptr();
        match style {
            TableStyle::CallerChecksSignature => Ok(Self {
                vec: Mutex::new(vec),
                maximum: table.maximum,
                table: *table,
                style: style.clone(),
                vm_table_definition: if let Some(table_loc) = vm_table_location {
                    {
                        let mut ptr = table_loc;
                        let td = ptr.as_mut();
                        td.base = base as _;
                        td.current_elements = table_minimum as _;
                    }
                    VMTableDefinitionOwnership::VMOwned(table_loc)
                } else {
                    VMTableDefinitionOwnership::HostOwned(Box::new(UnsafeCell::new(
                        VMTableDefinition {
                            base: base as _,
                            current_elements: table_minimum as _,
                        },
                    )))
                },
            }),
        }
    }

    /// Get the `VMTableDefinition`.
    ///
    /// # Safety
    /// - You must ensure that you have mutually exclusive access before calling
    ///   this function. You can get this by locking the `vec` mutex.
    unsafe fn get_vm_table_definition(&self) -> NonNull<VMTableDefinition> {
        match &self.vm_table_definition {
            VMTableDefinitionOwnership::VMOwned(ptr) => *ptr,
            VMTableDefinitionOwnership::HostOwned(boxed_ptr) => {
                NonNull::new_unchecked(boxed_ptr.get())
            }
        }
    }
}

impl Table for LinearTable {
    /// Returns the type for this Table.
    fn ty(&self) -> &TableType {
        &self.table
    }

    /// Returns the style for this Table.
    fn style(&self) -> &TableStyle {
        &self.style
    }

    /// Returns the number of allocated elements.
    fn size(&self) -> u32 {
        // TODO: investigate this function for race conditions
        unsafe {
            let td_ptr = self.get_vm_table_definition();
            let td = td_ptr.as_ref();
            td.current_elements
        }
    }

    /// Grow table by the specified amount of elements.
    ///
    /// Returns `None` if table can't be grown by the specified amount
    /// of elements, otherwise returns the previous size of the table.
    fn grow(&self, delta: u32) -> Option<u32> {
        let mut vec_guard = self.vec.lock().unwrap();
        let vec = vec_guard.borrow_mut();
        let size = self.size();
        let new_len = size.checked_add(delta)?;
        if self.maximum.map_or(false, |max| new_len > max) {
            return None;
        }
        vec.resize(usize::try_from(new_len).unwrap(), TableElement::default());

        // update table definition
        unsafe {
            let mut td_ptr = self.get_vm_table_definition();
            let td = td_ptr.as_mut();
            td.current_elements = new_len;
            td.base = vec.as_mut_ptr() as _;
        }
        Some(size)
    }

    /// Get reference to the specified element.
    ///
    /// Returns `None` if the index is out of bounds.
    fn get(&self, index: u32) -> Result<TableReference, Trap> {
        let vec_guard = self.vec.lock().unwrap();
        let raw_data = vec_guard
            .borrow()
            .get(index as usize)
            .cloned()
            .ok_or_else(|| Trap::new_from_runtime(TrapCode::TableAccessOutOfBounds))?;
        Ok(match self.table.ty {
            ValType::ExternRef => TableReference::ExternRef(unsafe { raw_data.extern_ref }),
            ValType::FuncRef => TableReference::FuncRef(unsafe { raw_data.func_ref }),
            _ => todo!("getting invalid type from table, handle this error"),
        })
    }

    /// Set reference to the specified element.
    ///
    /// # Errors
    ///
    /// Returns an error if the index is out of bounds.
    fn set(&self, index: u32, reference: TableReference) -> Result<(), Trap> {
        let mut vec_guard = self.vec.lock().unwrap();
        let vec = vec_guard.borrow_mut();
        match vec.get_mut(index as usize) {
            Some(slot) => {
                let element_data = match (self.table.ty, reference) {
                    (ValType::ExternRef, r @ TableReference::ExternRef(_)) => r.into(),
                    (ValType::FuncRef, r @ TableReference::FuncRef(_)) => r.into(),
                    // There is no trap code for this, are we supposed to statically verify that this can't happen?
                    _ => todo!("Trap if we set the wrong type"), //return Err(Trap::new_from_runtime(TrapCode::TableTypeMismatch))
                };
                *slot = element_data;
                Ok(())
            }
            None => Err(Trap::new_from_runtime(TrapCode::TableAccessOutOfBounds)),
        }
    }

    /// Return a `VMTableDefinition` for exposing the table to compiled wasm code.
    fn vmtable(&self) -> NonNull<VMTableDefinition> {
        let _vec_guard = self.vec.lock().unwrap();
        unsafe { self.get_vm_table_definition() }
    }
}
