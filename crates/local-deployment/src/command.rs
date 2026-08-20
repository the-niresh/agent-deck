use command_group::AsyncGroupChild;
use services::services::container::ContainerError;
use uuid::Uuid;

pub(crate) async fn kill_process_group(
    child: &mut AsyncGroupChild,
    run_id: Uuid,
) -> Result<(), ContainerError> {
    utils::process::kill_process_group(child, Some(run_id))
        .await
        .map_err(ContainerError::KillFailed)
}
