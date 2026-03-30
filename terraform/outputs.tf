output "environment" {
  description = "Production environment name"
  value       = github_repository_environment.prod.environment
}

output "repository" {
  description = "Repository name"
  value       = data.github_repository.repo.name
}
