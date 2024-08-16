use chickens::rocket;

#[rocket::main]
async fn main() -> Result<(), rocket::Error> {
    rocket().launch().await?;
    Ok(())
}
