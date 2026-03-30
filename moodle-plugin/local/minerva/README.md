# Minerva AI Assistant (local_minerva)

Local plugin that connects Moodle to a [Minerva](https://github.com/Edwinexd/minerva) instance for AI-assisted learning.

## Features

- **Course linking**: Teachers link a Moodle course to a Minerva course using an API key
- **Site-wide Minerva URL**: Admins can lock the Minerva instance URL so teachers only need to enter the API key
- **Enrolment sync**: Automatically syncs Moodle enrolments to Minerva on enrol/unenrol events
- **Material sync**: Upload course PDF files to Minerva for RAG processing (manual and scheduled)
- **Scheduled tasks**: Background sync of enrolments and materials

## Requirements

- Moodle 4.1 or later
- A running Minerva instance with an integration API key

## Installation

1. Download and extract the plugin into `local/minerva/`
2. Visit Site Administration > Notifications to complete the installation
3. Configure the Minerva URL in Site Administration > Plugins > Local plugins > Minerva AI Assistant

## Related plugin

This plugin works together with [mod_minerva](https://moodle.org/plugins/mod_minerva), which provides the activity module that teachers add to their course page.

## License

This plugin is licensed under the [GNU GPL v3 or later](https://www.gnu.org/copyleft/gpl.html).
