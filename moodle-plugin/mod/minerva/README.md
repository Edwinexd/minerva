# Minerva AI Assistant (mod_minerva)

Activity module that embeds a [Minerva](https://github.com/Edwinexd/minerva) AI chat assistant into a Moodle course page.

## Features

- **Course page activity**: Teachers add the Minerva AI Assistant as a standard Moodle activity
- **Embedded chat**: Students access the AI assistant directly from the course page via an iframe
- **Token-based auth**: Each view generates a scoped embed token so students don't need separate logins
- **Open in new tab**: Option to open the chat in a full browser tab

## Requirements

- Moodle 4.1 or later
- [local_minerva](https://moodle.org/plugins/local_minerva) plugin installed and configured
- A running Minerva instance linked to the course

## Installation

1. Install [local_minerva](https://moodle.org/plugins/local_minerva) first
2. Download and extract this plugin into `mod/minerva/`
3. Visit Site Administration > Notifications to complete the installation
4. Link a course to Minerva via the course navigation "Minerva settings"
5. Add "Minerva AI Assistant" as an activity on the course page

## License

This plugin is licensed under the [GNU GPL v3 or later](https://www.gnu.org/copyleft/gpl.html).
