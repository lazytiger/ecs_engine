use dataproxy::Table;
use mysql::{Opts, Pool};

fn run() -> mysql::Result<()> {
    let url = "mysql://game:BabelTime@192.168.176.145:4000/mysql";
    let opts = Opts::from_url(url)?;
    let pool = Pool::new(opts)?;
    let mut conn = pool.get_conn()?;
    let table = Table::new("mysql", "user", &mut conn)?;
    println!("{:?}", table);
    Ok(())
}
fn main() {
    run().unwrap();
}
