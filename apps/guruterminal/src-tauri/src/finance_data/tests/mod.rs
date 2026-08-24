use super::{world_bank::*, *};

fn query() -> ValidatedMacroDataQuery {
    ValidatedMacroDataQuery {
        economy: "USA".to_owned(),
        indicator: "NY.GDP.MKTP.CD".to_owned(),
        start_year: 2020,
        end_year: 2021,
    }
}

mod requests;
