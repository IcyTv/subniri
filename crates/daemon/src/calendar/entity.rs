use sea_orm::entity::prelude::*;

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "calendar_events")]
pub struct Model {
	#[sea_orm(primary_key, auto_increment = false)]
	pub id: uuid::Uuid,
	pub title: String,
	pub description: Option<String>,
	pub start_time: chrono::NaiveDateTime,
	pub end_time: chrono::NaiveDateTime,
	pub is_all_day: bool,
	pub color: Option<String>,
	#[sea_orm(default_value = 15)]
	pub reminder_minutes: Option<i32>,
}

impl ActiveModelBehavior for ActiveModel {}
