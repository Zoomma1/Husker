use bollard::Docker;

pub async fn ping_daemon(docker: &Docker) -> Result<(), bollard::errors::Error> {
    docker.ping().await?;
    Ok(())
}

pub(crate) async fn create_network(docker: &Docker, network_name: &str) -> Result<(), bollard::errors::Error> {
    let options = bollard::models::NetworkCreateRequest {
        name: network_name.to_string(),
        ..Default::default()
    };
    docker.create_network(options).await?;
    Ok(())
}

pub(crate) async fn delete_network(docker: &Docker, network_name: &str) -> Result<(), bollard::errors::Error> {
    docker.remove_network(network_name).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn test_ping_daemon() {
        let docker = Docker::connect_with_local_defaults().unwrap();
        assert!(ping_daemon(&docker).await.is_ok());
    }
}