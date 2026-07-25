pub use api_types::InvitationStatus;
use api_types::MemberRole;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use super::{
    identity_errors::IdentityError,
    organization_members::{add_member, assert_admin},
    organizations::{Organization, OrganizationRepository, is_personal_org},
};
use crate::db::organization_members::is_member;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Invitation {
    pub id: Uuid,
    pub organization_id: Uuid,
    pub invited_by_user_id: Option<Uuid>,
    pub email: String,
    pub role: MemberRole,
    pub status: InvitationStatus,
    pub token: String,
    pub expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub struct InvitationRepository<'a> {
    pool: &'a PgPool,
}

impl<'a> InvitationRepository<'a> {
    pub fn new(pool: &'a PgPool) -> Self {
        Self { pool }
    }

    pub async fn create_invitation(
        &self,
        organization_id: Uuid,
        invited_by_user_id: Uuid,
        email: &str,
        role: MemberRole,
        expires_at: DateTime<Utc>,
        token: &str,
    ) -> Result<Invitation, IdentityError> {
        assert_admin(self.pool, organization_id, invited_by_user_id).await?;

        if OrganizationRepository::new(self.pool)
            .is_personal(organization_id)
            .await?
        {
            return Err(IdentityError::InvitationError(
                "Cannot invite members to a personal organization".to_string(),
            ));
        }

        let invitation = sqlx::query_as!(
            Invitation,
            r#"
            INSERT INTO organization_invitations (
                organization_id, invited_by_user_id, email, role, token, expires_at
            )
            VALUES ($1, $2, $3, $4, $5, $6)
            RETURNING
                id AS "id!",
                organization_id AS "organization_id!: Uuid",
                invited_by_user_id AS "invited_by_user_id?: Uuid",
                email AS "email!",
                role AS "role!: MemberRole",
                status AS "status!: InvitationStatus",
                token AS "token!",
                expires_at AS "expires_at!",
                created_at AS "created_at!",
                updated_at AS "updated_at!"
            "#,
            organization_id,
            invited_by_user_id,
            email,
            role as MemberRole,
            token,
            expires_at
        )
        .fetch_one(self.pool)
        .await
        .map_err(|e| {
            if let Some(db_err) = e.as_database_error()
                && db_err.is_unique_violation()
            {
                return IdentityError::InvitationError(
                    "A pending invitation already exists for this email".to_string(),
                );
            }
            IdentityError::from(e)
        })?;

        Ok(invitation)
    }

    pub async fn list_invitations(
        &self,
        organization_id: Uuid,
        requesting_user_id: Uuid,
    ) -> Result<Vec<Invitation>, IdentityError> {
        assert_admin(self.pool, organization_id, requesting_user_id).await?;

        if OrganizationRepository::new(self.pool)
            .is_personal(organization_id)
            .await?
        {
            return Err(IdentityError::InvitationError(
                "Personal organizations do not support invitations".to_string(),
            ));
        }

        let invitations = sqlx::query_as!(
            Invitation,
            r#"
            SELECT
                id AS "id!",
                organization_id AS "organization_id!: Uuid",
                invited_by_user_id AS "invited_by_user_id?: Uuid",
                email AS "email!",
                role AS "role!: MemberRole",
                status AS "status!: InvitationStatus",
                token AS "token!",
                expires_at AS "expires_at!",
                created_at AS "created_at!",
                updated_at AS "updated_at!"
            FROM organization_invitations
            WHERE organization_id = $1
            ORDER BY created_at DESC
            "#,
            organization_id
        )
        .fetch_all(self.pool)
        .await?;

        Ok(invitations)
    }

    /// Whether `email` has an invitation that is still pending and unexpired.
    ///
    /// Used to admit invited people through OAuth signup without adding every
    /// one of them to a static allowlist.
    pub async fn has_pending_invitation(&self, email: &str) -> Result<bool, IdentityError> {
        let normalized = email.trim().to_ascii_lowercase();
        let exists = sqlx::query_scalar!(
            r#"
            SELECT EXISTS (
                SELECT 1
                FROM organization_invitations
                WHERE lower(email) = $1
                  AND status = 'pending'
                  AND expires_at > now()
            ) AS "exists!"
            "#,
            normalized
        )
        .fetch_one(self.pool)
        .await?;

        Ok(exists)
    }

    pub async fn get_invitation_by_token(&self, token: &str) -> Result<Invitation, IdentityError> {
        sqlx::query_as!(
            Invitation,
            r#"
            SELECT
                id AS "id!",
                organization_id AS "organization_id!: Uuid",
                invited_by_user_id AS "invited_by_user_id?: Uuid",
                email AS "email!",
                role AS "role!: MemberRole",
                status AS "status!: InvitationStatus",
                token AS "token!",
                expires_at AS "expires_at!",
                created_at AS "created_at!",
                updated_at AS "updated_at!"
            FROM organization_invitations
            WHERE token = $1
            "#,
            token
        )
        .fetch_optional(self.pool)
        .await?
        .ok_or(IdentityError::NotFound)
    }

    pub async fn revoke_invitation(
        &self,
        organization_id: Uuid,
        invitation_id: Uuid,
        requesting_user_id: Uuid,
    ) -> Result<(), IdentityError> {
        assert_admin(self.pool, organization_id, requesting_user_id).await?;

        let result = sqlx::query!(
            r#"
            DELETE FROM organization_invitations
            WHERE id = $1 AND organization_id = $2
            "#,
            invitation_id,
            organization_id
        )
        .execute(self.pool)
        .await?;

        if result.rows_affected() == 0 {
            return Err(IdentityError::NotFound);
        }

        Ok(())
    }

    /// Accept an invitation on behalf of `user_id`.
    ///
    /// `user_email` is required and must match the address the invitation was
    /// issued to. The token alone is NOT sufficient authorization: invitation
    /// links travel through email and get forwarded, so without this binding
    /// anyone holding the token could join the organization with the invited
    /// role — including Admin.
    pub async fn accept_invitation(
        &self,
        token: &str,
        user_id: Uuid,
        user_email: &str,
    ) -> Result<(Organization, MemberRole), IdentityError> {
        let mut tx = super::begin_tx(self.pool).await?;

        let invitation = sqlx::query_as!(
            Invitation,
            r#"
            SELECT
                id AS "id!",
                organization_id AS "organization_id!: Uuid",
                invited_by_user_id AS "invited_by_user_id?: Uuid",
                email AS "email!",
                role AS "role!: MemberRole",
                status AS "status!: InvitationStatus",
                token AS "token!",
                expires_at AS "expires_at!",
                created_at AS "created_at!",
                updated_at AS "updated_at!"
            FROM organization_invitations
            WHERE token = $1 AND status = 'pending'
            FOR UPDATE
            "#,
            token
        )
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| {
            IdentityError::InvitationError("Invitation not found or already used".to_string())
        })?;

        // Bind the invitation to the identity it was issued to. Checked before
        // any other validation so a mismatched caller cannot cause state
        // changes (such as the expiry transition below).
        if !invitation.email.eq_ignore_ascii_case(user_email.trim()) {
            tx.rollback().await?;
            return Err(IdentityError::InvitationError(
                "This invitation was issued to a different email address. \
                 Sign in as the invited user to accept it."
                    .to_string(),
            ));
        }

        if is_personal_org(&mut *tx, invitation.organization_id).await? {
            tx.rollback().await?;
            return Err(IdentityError::InvitationError(
                "Cannot accept invitations for a personal organization".to_string(),
            ));
        }

        if invitation.expires_at < Utc::now() {
            sqlx::query!(
                r#"
                UPDATE organization_invitations
                SET status = 'expired'
                WHERE id = $1
                "#,
                invitation.id
            )
            .execute(&mut *tx)
            .await?;

            tx.commit().await?;
            return Err(IdentityError::InvitationError(
                "Invitation has expired".to_string(),
            ));
        }

        if is_member(&mut *tx, invitation.organization_id, user_id).await? {
            tx.rollback().await?;
            return Err(IdentityError::InvitationError(
                "You are already a member of the organization".to_string(),
            ));
        }

        add_member(
            &mut *tx,
            invitation.organization_id,
            user_id,
            invitation.role,
        )
        .await?;

        sqlx::query!(
            r#"
            UPDATE organization_invitations
            SET status = 'accepted'
            WHERE id = $1
            "#,
            invitation.id
        )
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;

        let organization = OrganizationRepository::new(self.pool)
            .fetch_organization(invitation.organization_id)
            .await?;

        Ok((organization, invitation.role))
    }
}
