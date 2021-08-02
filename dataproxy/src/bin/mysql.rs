use dataproxy::{BoolValue, Column, Table};
use mysql::{prelude::Queryable, Opts, Params, Pool};

fn run() -> mysql::Result<()> {
    let url = "mysql://game:BabelTime@192.168.176.145:4000/game";
    let opts = Opts::from_url(url)?;
    let pool = Pool::new(opts)?;
    let mut conn = pool.get_conn()?;
    let table = Table::new(None, "user", &mut conn)?;
    let mut new_table = Table::default();
    new_table.set_engine("InnoDb");
    new_table.set_name("user");
    new_table.set_charset("utf8mb4");

    let mut column = Column::default();
    column.field = "x".into();
    column.field_type = "smallint(5) unsigned".into();
    column.default = None;
    column.null = BoolValue::No;
    new_table.columns.push(column);

    let mut column = Column::default();
    column.field = "y".into();
    column.field_type = "smallint(5) unsigned".into();
    column.default = None;
    column.null = BoolValue::No;
    new_table.columns.push(column);

    let result = new_table.diff(&table).unwrap();
    for r in result {
        println!("{}", r);
        conn.exec_drop(r, Params::Empty).unwrap();
    }
    Ok(())
}
fn main() {
    run().unwrap();
}
