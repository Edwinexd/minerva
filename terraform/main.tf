terraform {
  required_version = ">= 1.0"

  required_providers {
    github = {
      source  = "integrations/github"
      version = "~> 6.0"
    }
  }
}

provider "github" {
  owner = var.github_owner
}

data "github_repository" "repo" {
  full_name = "${var.github_owner}/${var.github_repo}"
}

# Production Environment
resource "github_repository_environment" "prod" {
  repository  = data.github_repository.repo.name
  environment = "prod"
}

# =============================================================================
# Generated k8s secrets
# =============================================================================

locals {
  k8s_secrets_yaml = <<-YAML
    apiVersion: v1
    kind: Secret
    metadata:
      name: minerva-secrets
      namespace: minerva
    type: Opaque
    stringData:
      POSTGRES_USER: "${var.postgres_user}"
      POSTGRES_PASSWORD: "${var.postgres_password}"
      DATABASE_URL: "postgres://${var.postgres_user}:${var.postgres_password}@postgres:5432/minerva"
      MINERVA_HMAC_SECRET: "${var.minerva_hmac_secret}"
      MINERVA_ADMINS: "${var.minerva_admins}"
      CEREBRAS_API_KEY: "${var.cerebras_api_key}"
      OPENAI_API_KEY: "${var.openai_api_key}"
      MINERVA_SERVICE_API_KEY: "${var.minerva_service_api_key}"
  YAML
}

# =============================================================================
# Prod Environment Secrets (infra repo)
# =============================================================================

resource "github_actions_environment_secret" "prod_kubeconfig" {
  repository      = data.github_repository.repo.name
  environment     = github_repository_environment.prod.environment
  secret_name     = "KUBECONFIG"
  plaintext_value = var.kubeconfig
}

resource "github_actions_environment_secret" "prod_wireguard_private_key" {
  repository      = data.github_repository.repo.name
  environment     = github_repository_environment.prod.environment
  secret_name     = "WIREGUARD_PRIVATE_KEY"
  plaintext_value = var.wireguard_private_key
}

resource "github_actions_environment_secret" "prod_wireguard_config" {
  repository      = data.github_repository.repo.name
  environment     = github_repository_environment.prod.environment
  secret_name     = "WIREGUARD_CONFIG"
  plaintext_value = var.wireguard_config
}

resource "github_actions_environment_secret" "prod_ssh_private_key" {
  repository      = data.github_repository.repo.name
  environment     = github_repository_environment.prod.environment
  secret_name     = "SSH_PRIVATE_KEY"
  plaintext_value = var.ssh_private_key
}

resource "github_actions_environment_secret" "prod_k8s_secrets" {
  repository      = data.github_repository.repo.name
  environment     = github_repository_environment.prod.environment
  secret_name     = "K8S_SECRETS"
  plaintext_value = base64encode(local.k8s_secrets_yaml)
}

# =============================================================================
# Repository Secrets (transcript pipeline)
# =============================================================================

resource "github_actions_secret" "service_api_key" {
  repository      = data.github_repository.repo.name
  secret_name     = "MINERVA_SERVICE_API_KEY"
  plaintext_value = var.minerva_service_api_key
}

resource "github_actions_secret" "su_username" {
  repository      = data.github_repository.repo.name
  secret_name     = "SU_USERNAME"
  plaintext_value = var.su_username
}

resource "github_actions_secret" "su_password" {
  repository      = data.github_repository.repo.name
  secret_name     = "SU_PASSWORD"
  plaintext_value = var.su_password
}
