use crate::entities::FieldType;
use crate::services::cell::{
  CellCache, CellDataChangeset, CellDataDecoder, CellFilterCache, CellProtobufBlob,
  FromCellChangesetString,
};
use crate::services::field::{
  CheckboxTypeOption, ChecklistTypeOption, DateTypeOption, MultiSelectTypeOption, NumberTypeOption,
  RichTextTypeOption, SingleSelectTypeOption, TypeOption, TypeOptionCellData,
  TypeOptionCellDataCompare, TypeOptionCellDataFilter, TypeOptionTransform, URLTypeOption,
};
use crate::services::filter::FilterType;
use collab_database::fields::{Field, TypeOptionData};
use collab_database::rows::Cell;
use flowy_error::FlowyResult;
use serde::Serialize;
use std::any::Any;
use std::cmp::Ordering;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// A helper trait that used to erase the `Self` of `TypeOption` trait to make it become a Object-safe trait
/// Only object-safe traits can be made into trait objects.
/// > Object-safe traits are traits with methods that follow these two rules:
/// 1.the return type is not Self.
/// 2.there are no generic types parameters.
///
pub trait TypeOptionCellDataHandler {
  fn handle_cell_str(
    &self,
    cell: &Cell,
    decoded_field_type: &FieldType,
    field_rev: &Field,
  ) -> FlowyResult<CellProtobufBlob>;

  fn handle_cell_changeset(
    &self,
    cell_changeset: String,
    old_cell: Option<Cell>,
    field: &Field,
  ) -> FlowyResult<Cell>;

  fn handle_cell_compare(&self, left_cell: &Cell, right_cell: &Cell, field: &Field) -> Ordering;

  fn handle_cell_filter(&self, filter_type: &FilterType, field: &Field, cell: &Cell) -> bool;

  /// Decode the cell_str to corresponding cell data, and then return the display string of the
  /// cell data.
  fn stringify_cell_str(
    &self,
    cell: &Cell,
    decoded_field_type: &FieldType,
    field: &Field,
  ) -> String;

  fn get_cell_data(
    &self,
    cell: &Cell,
    decoded_field_type: &FieldType,
    field_rev: &Field,
  ) -> FlowyResult<BoxCellData>;
}

struct CellDataCacheKey(u64);
impl CellDataCacheKey {
  pub fn new(field_rev: &Field, decoded_field_type: FieldType, cell: &Cell) -> Self {
    let mut hasher = DefaultHasher::new();
    if let Some(type_option_data) = field_rev.get_any_type_option(&decoded_field_type) {
      type_option_data.hash(&mut hasher);
    }
    hasher.write(field_rev.id.as_bytes());
    hasher.write_u8(decoded_field_type as u8);
    cell.hash(&mut hasher);
    Self(hasher.finish())
  }
}

impl AsRef<u64> for CellDataCacheKey {
  fn as_ref(&self) -> &u64 {
    &self.0
  }
}

struct TypeOptionCellDataHandlerImpl<T> {
  inner: T,
  cell_data_cache: Option<CellCache>,
  cell_filter_cache: Option<CellFilterCache>,
}

impl<T> TypeOptionCellDataHandlerImpl<T>
where
  T: TypeOption
    + CellDataDecoder
    + CellDataChangeset
    + TypeOptionCellData
    + TypeOptionTransform
    + TypeOptionCellDataFilter
    + TypeOptionCellDataCompare
    + 'static,
{
  pub fn new_with_boxed(
    inner: T,
    cell_filter_cache: Option<CellFilterCache>,
    cell_data_cache: Option<CellCache>,
  ) -> Box<dyn TypeOptionCellDataHandler> {
    Box::new(Self {
      inner,
      cell_data_cache,
      cell_filter_cache,
    }) as Box<dyn TypeOptionCellDataHandler>
  }
}

impl<T> TypeOptionCellDataHandlerImpl<T>
where
  T: TypeOption + CellDataDecoder,
{
  fn get_decoded_cell_data(
    &self,
    cell: &Cell,
    decoded_field_type: &FieldType,
    field: &Field,
  ) -> FlowyResult<<Self as TypeOption>::CellData> {
    let key = CellDataCacheKey::new(field, decoded_field_type.clone(), &cell);
    if let Some(cell_data_cache) = self.cell_data_cache.as_ref() {
      let read_guard = cell_data_cache.read();
      if let Some(cell_data) = read_guard.get(key.as_ref()).cloned() {
        tracing::trace!(
          "Cell cache hit: field_type:{}, cell: {:?}, cell_data: {:?}",
          decoded_field_type,
          cell,
          cell_data
        );
        return Ok(cell_data);
      }
    }

    let cell_data = self.decode_cell_str(cell, decoded_field_type, field)?;
    if let Some(cell_data_cache) = self.cell_data_cache.as_ref() {
      tracing::trace!(
        "Cell cache update: field_type:{}, cell: {:?}, cell_data: {:?}",
        decoded_field_type,
        cell,
        cell_data
      );
      cell_data_cache
        .write()
        .insert(key.as_ref(), cell_data.clone());
    }
    Ok(cell_data)
  }

  fn set_decoded_cell_data(
    &self,
    cell: &Cell,
    cell_data: <Self as TypeOption>::CellData,
    field: &Field,
  ) {
    if let Some(cell_data_cache) = self.cell_data_cache.as_ref() {
      let field_type = FieldType::from(field.field_type);
      let key = CellDataCacheKey::new(field, field_type.clone(), cell);
      tracing::trace!(
        "Cell cache update: field_type:{}, cell: {:?}, cell_data: {:?}",
        field_type,
        cell,
        cell_data
      );
      cell_data_cache.write().insert(key.as_ref(), cell_data);
    }
  }
}

impl<T> std::ops::Deref for TypeOptionCellDataHandlerImpl<T> {
  type Target = T;

  fn deref(&self) -> &Self::Target {
    &self.inner
  }
}

impl<T> TypeOption for TypeOptionCellDataHandlerImpl<T>
where
  T: TypeOption,
{
  type CellData = T::CellData;
  type CellChangeset = T::CellChangeset;
  type CellProtobufType = T::CellProtobufType;
  type CellFilter = T::CellFilter;
}

impl<T> TypeOptionCellDataHandler for TypeOptionCellDataHandlerImpl<T>
where
  T: TypeOption
    + CellDataDecoder
    + CellDataChangeset
    + TypeOptionCellData
    + TypeOptionTransform
    + TypeOptionCellDataFilter
    + TypeOptionCellDataCompare,
{
  fn handle_cell_str(
    &self,
    cell: &Cell,
    decoded_field_type: &FieldType,
    field_rev: &Field,
  ) -> FlowyResult<CellProtobufBlob> {
    let cell_data = self
      .get_cell_data(cell, decoded_field_type, field_rev)?
      .unbox_or_default::<<Self as TypeOption>::CellData>();

    CellProtobufBlob::from(self.convert_to_protobuf(cell_data))
  }

  fn handle_cell_changeset(
    &self,
    cell_changeset: String,
    old_cell: Option<Cell>,
    field: &Field,
  ) -> FlowyResult<Cell> {
    let changeset = <Self as TypeOption>::CellChangeset::from_changeset(cell_changeset)?;
    let (cell, cell_data) = self.apply_changeset(changeset, old_cell)?;
    self.set_decoded_cell_data(&cell, cell_data, field);
    Ok(cell)
  }

  fn handle_cell_compare(&self, left_cell: &Cell, right_cell: &Cell, field: &Field) -> Ordering {
    let field_type = FieldType::from(field.field_type);
    let left = self
      .get_decoded_cell_data(left_cell, &field_type, field)
      .unwrap_or_default();
    let right = self
      .get_decoded_cell_data(right_cell, &field_type, field)
      .unwrap_or_default();
    self.apply_cmp(&left, &right)
  }

  fn handle_cell_filter(&self, filter_type: &FilterType, field: &Field, cell: &Cell) -> bool {
    let perform_filter = || {
      let filter_cache = self.cell_filter_cache.as_ref()?.read();
      let cell_filter = filter_cache.get::<<Self as TypeOption>::CellFilter>(filter_type)?;
      let cell_data = self
        .get_decoded_cell_data(cell, &filter_type.field_type, field)
        .ok()?;
      Some(self.apply_filter(cell_filter, &filter_type.field_type, &cell_data))
    };

    perform_filter().unwrap_or(true)
  }

  fn stringify_cell_str(
    &self,
    cell: &Cell,
    decoded_field_type: &FieldType,
    field: &Field,
  ) -> String {
    if self.transformable() {
      let cell_data = self.transform_type_option_cell(cell, decoded_field_type, field);
      if let Some(cell_data) = cell_data {
        return self.decode_cell_data_to_str(cell_data);
      }
    }
    self.decode_cell_to_str(cell)
  }

  fn get_cell_data(
    &self,
    cell: &Cell,
    decoded_field_type: &FieldType,
    field_rev: &Field,
  ) -> FlowyResult<BoxCellData> {
    // tracing::debug!("get_cell_data: {:?}", std::any::type_name::<Self>());
    let cell_data = if self.transformable() {
      match self.transform_type_option_cell(&cell, decoded_field_type, field_rev) {
        None => self.get_decoded_cell_data(cell, decoded_field_type, field_rev)?,
        Some(cell_data) => cell_data,
      }
    } else {
      self.get_decoded_cell_data(cell, decoded_field_type, field_rev)?
    };
    Ok(BoxCellData::new(cell_data))
  }
}

pub struct TypeOptionCellExt<'a> {
  field: &'a Field,
  cell_data_cache: Option<CellCache>,
  cell_filter_cache: Option<CellFilterCache>,
}

impl<'a> TypeOptionCellExt<'a> {
  pub fn new_with_cell_data_cache(field: &'a Field, cell_data_cache: Option<CellCache>) -> Self {
    Self {
      field,
      cell_data_cache,
      cell_filter_cache: None,
    }
  }

  pub fn new(
    field: &'a Field,
    cell_data_cache: Option<CellCache>,
    cell_filter_cache: Option<CellFilterCache>,
  ) -> Self {
    let mut this = Self::new_with_cell_data_cache(field, cell_data_cache);
    this.cell_filter_cache = cell_filter_cache;
    this
  }

  pub fn get_cells<T>(&self) -> Vec<T> {
    let field_type = FieldType::from(self.field.field_type);
    match self.get_type_option_cell_data_handler(&field_type) {
      None => vec![],
      Some(_handler) => {
        todo!()
      },
    }
  }

  pub fn get_type_option_cell_data_handler(
    &self,
    field_type: &FieldType,
  ) -> Option<Box<dyn TypeOptionCellDataHandler>> {
    match field_type {
      FieldType::RichText => self
        .field
        .get_type_option::<RichTextTypeOption>(field_type)
        .map(|type_option| {
          TypeOptionCellDataHandlerImpl::new_with_boxed(
            type_option,
            self.cell_filter_cache.clone(),
            self.cell_data_cache.clone(),
          )
        }),
      FieldType::Number => self
        .field
        .get_type_option::<NumberTypeOption>(field_type)
        .map(|type_option| {
          TypeOptionCellDataHandlerImpl::new_with_boxed(
            type_option,
            self.cell_filter_cache.clone(),
            self.cell_data_cache.clone(),
          )
        }),
      FieldType::DateTime => self
        .field
        .get_type_option::<DateTypeOption>(field_type)
        .map(|type_option| {
          TypeOptionCellDataHandlerImpl::new_with_boxed(
            type_option,
            self.cell_filter_cache.clone(),
            self.cell_data_cache.clone(),
          )
        }),
      FieldType::SingleSelect => self
        .field
        .get_type_option::<SingleSelectTypeOption>(field_type)
        .map(|type_option| {
          TypeOptionCellDataHandlerImpl::new_with_boxed(
            type_option,
            self.cell_filter_cache.clone(),
            self.cell_data_cache.clone(),
          )
        }),
      FieldType::MultiSelect => self
        .field
        .get_type_option::<MultiSelectTypeOption>(field_type)
        .map(|type_option| {
          TypeOptionCellDataHandlerImpl::new_with_boxed(
            type_option,
            self.cell_filter_cache.clone(),
            self.cell_data_cache.clone(),
          )
        }),
      FieldType::Checkbox => self
        .field
        .get_type_option::<CheckboxTypeOption>(field_type)
        .map(|type_option| {
          TypeOptionCellDataHandlerImpl::new_with_boxed(
            type_option,
            self.cell_filter_cache.clone(),
            self.cell_data_cache.clone(),
          )
        }),
      FieldType::URL => {
        self
          .field
          .get_type_option::<URLTypeOption>(field_type)
          .map(|type_option| {
            TypeOptionCellDataHandlerImpl::new_with_boxed(
              type_option,
              self.cell_filter_cache.clone(),
              self.cell_data_cache.clone(),
            )
          })
      },
      FieldType::Checklist => self
        .field
        .get_type_option::<ChecklistTypeOption>(field_type)
        .map(|type_option| {
          TypeOptionCellDataHandlerImpl::new_with_boxed(
            type_option,
            self.cell_filter_cache.clone(),
            self.cell_data_cache.clone(),
          )
        }),
    }
  }
}

pub fn transform_type_option(
  type_option_data: &TypeOptionData,
  new_field_type: &FieldType,
  old_type_option_data: Option<TypeOptionData>,
  old_field_type: FieldType,
) -> String {
  let mut transform_handler = get_type_option_transform_handler(type_option_data, new_field_type);
  if let Some(old_type_option_data) = old_type_option_data {
    transform_handler.transform(old_field_type, old_type_option_data);
  }
  transform_handler.json_str()
}

/// A helper trait that used to erase the `Self` of `TypeOption` trait to make it become a Object-safe trait.
pub trait TypeOptionTransformHandler {
  fn transform(
    &mut self,
    old_type_option_field_type: FieldType,
    old_type_option_data: TypeOptionData,
  );

  fn json_str(&self) -> String;
}

impl<T> TypeOptionTransformHandler for T
where
  T: TypeOptionTransform + Serialize,
{
  fn transform(
    &mut self,
    old_type_option_field_type: FieldType,
    old_type_option_data: TypeOptionData,
  ) {
    if self.transformable() {
      self.transform_type_option(old_type_option_field_type, old_type_option_data)
    }
  }

  fn json_str(&self) -> String {
    serde_json::to_string(&self).unwrap()
  }
}
fn get_type_option_transform_handler(
  type_option_data: &TypeOptionData,
  field_type: &FieldType,
) -> Box<dyn TypeOptionTransformHandler> {
  let type_option_data = type_option_data.clone();
  match field_type {
    FieldType::RichText => {
      Box::new(RichTextTypeOption::from(type_option_data)) as Box<dyn TypeOptionTransformHandler>
    },
    FieldType::Number => {
      Box::new(NumberTypeOption::from(type_option_data)) as Box<dyn TypeOptionTransformHandler>
    },
    FieldType::DateTime => {
      Box::new(DateTypeOption::from(type_option_data)) as Box<dyn TypeOptionTransformHandler>
    },
    FieldType::SingleSelect => Box::new(SingleSelectTypeOption::from(type_option_data))
      as Box<dyn TypeOptionTransformHandler>,
    FieldType::MultiSelect => {
      Box::new(MultiSelectTypeOption::from(type_option_data)) as Box<dyn TypeOptionTransformHandler>
    },
    FieldType::Checkbox => {
      Box::new(CheckboxTypeOption::from(type_option_data)) as Box<dyn TypeOptionTransformHandler>
    },
    FieldType::URL => {
      Box::new(URLTypeOption::from(type_option_data)) as Box<dyn TypeOptionTransformHandler>
    },
    FieldType::Checklist => {
      Box::new(ChecklistTypeOption::from(type_option_data)) as Box<dyn TypeOptionTransformHandler>
    },
  }
}

pub struct BoxCellData(Box<dyn Any + Send + Sync + 'static>);

impl BoxCellData {
  fn new<T>(value: T) -> Self
  where
    T: Send + Sync + 'static,
  {
    Self(Box::new(value))
  }

  fn unbox_or_default<T>(self) -> T
  where
    T: Default + 'static,
  {
    match self.0.downcast::<T>() {
      Ok(value) => *value,
      Err(_) => T::default(),
    }
  }

  pub(crate) fn unbox_or_none<T>(self) -> Option<T>
  where
    T: Default + 'static,
  {
    match self.0.downcast::<T>() {
      Ok(value) => Some(*value),
      Err(_) => None,
    }
  }

  #[allow(dead_code)]
  fn downcast_ref<T: 'static>(&self) -> Option<&T> {
    self.0.downcast_ref()
  }
}

pub struct RowSingleCellData {
  pub row_id: String,
  pub field_id: String,
  pub field_type: FieldType,
  pub cell_data: BoxCellData,
}

macro_rules! into_cell_data {
  ($func_name:ident,$return_ty:ty) => {
    #[allow(dead_code)]
    pub fn $func_name(self) -> Option<$return_ty> {
      self.cell_data.unbox_or_none()
    }
  };
}

impl RowSingleCellData {
  into_cell_data!(
    into_text_field_cell_data,
    <RichTextTypeOption as TypeOption>::CellData
  );
  into_cell_data!(
    into_number_field_cell_data,
    <NumberTypeOption as TypeOption>::CellData
  );
  into_cell_data!(
    into_url_field_cell_data,
    <URLTypeOption as TypeOption>::CellData
  );
  into_cell_data!(
    into_single_select_field_cell_data,
    <SingleSelectTypeOption as TypeOption>::CellData
  );
  into_cell_data!(
    into_multi_select_field_cell_data,
    <MultiSelectTypeOption as TypeOption>::CellData
  );
  into_cell_data!(
    into_date_field_cell_data,
    <DateTypeOption as TypeOption>::CellData
  );
  into_cell_data!(
    into_check_list_field_cell_data,
    <CheckboxTypeOption as TypeOption>::CellData
  );
}
