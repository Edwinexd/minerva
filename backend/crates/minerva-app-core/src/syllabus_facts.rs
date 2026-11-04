//! Facts read off an SU course plan PDF: its level and the modules it is
//! examined in. Both sit in fixed places of the plan template, in Swedish
//! and in English:
//!
//! ```text
//! Fördjupning:      G2F - Grundnivå, har minst 60 hp ...      (sv)
//! Specialisation:   G1N - First cycle, has only upper-...     (en)
//!
//! Provkod   Benämning                      Högskolepoäng      (sv)
//! Examination code  Name          Higher Education Credits    (en)
//! PR2T      Programmering 2, tenta                     4
//! PR2I      Programmering 2, inlämningsuppgift       3.5
//! ```
//!
//! The text must come from `pdftotext -layout`
//! (`minerva_pipeline::pdf::extract_pages`), which keeps a table row on
//! one line. Anything the parser does not recognise is left out rather
//! than guessed; the assessment then says the plan does not state it.

use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PlanFacts {
    /// Level code, e.g. `G1N`, `G2F`, `A1N`.
    pub level: Option<String>,
    pub modules: Vec<PlanModule>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanModule {
    /// Ladok examination code, e.g. `PR2T`.
    pub code: String,
    pub name: String,
    pub credits: f64,
}

static LEVEL_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^\s*(?:Fördjupning|Specialisation|Level)\s*:\s+([GA][0-9X][A-Z]{1,2})\b")
        .expect("level regex")
});

/// Header row of the module table.
static MODULE_HEADER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*(?:Provkod|Examination code)\s{2,}").expect("header regex"));

/// `Sidan 1/2` / `Page 1/2`, printed at the foot of every page.
static PAGE_FOOTER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*(?:Sidan|Page)\s+\d+/\d+\s*$").expect("footer regex"));

/// One module row: code, name, credits, separated by runs of spaces.
static MODULE_ROW_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*([A-Z0-9]{2,8})\s{2,}(\S.*?)\s{2,}(\d+(?:[.,]\d+)?)\s*$").expect("row regex")
});

pub fn parse(pages: &[String]) -> PlanFacts {
    let text = pages.join("\n");
    let level = LEVEL_RE.captures(&text).map(|c| c[1].to_string());

    let mut modules = Vec::new();
    let mut in_table = false;
    for line in text.lines() {
        if !in_table {
            in_table = MODULE_HEADER_RE.is_match(line);
            continue;
        }
        // Blank lines and a page footer can sit inside the table when it
        // runs over a page break; the table ends at the next heading.
        if line.trim().is_empty() || PAGE_FOOTER_RE.is_match(line) {
            continue;
        }
        match MODULE_ROW_RE.captures(line) {
            Some(c) => {
                let Ok(credits) = c[3].replace(',', ".").parse::<f64>() else {
                    break;
                };
                modules.push(PlanModule {
                    code: c[1].to_string(),
                    name: c[2].trim().to_string(),
                    credits,
                });
            }
            None => break,
        }
    }
    PlanFacts { level, modules }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SV: &str = "Kursplan
för kurs på grundnivå
Programmering 2                                                                          7.5 Högskolepoäng
Programming 2                                                                            7.5 ECTS credits
Kurskod:                                        IB440C
Gäller från:                                    HT 2020
Huvudområde:                                    Data- och systemvetenskap
Fördjupning:                                    G2F - Grundnivå, har minst 60 hp kurs/er på grundnivå som förkunskapskrav

Beslut
Kursens uppläggning
Provkod               Benämning                                                                             Högskolepoäng
PR2T                  Programmering 2, tenta                                                                             4
PR2I                  Programmering 2, inlämningsuppgift                                                               3.5

Kursens innehåll
Kursen förutsätter grundläggande kunskaper i programmering med Java från PROG1 eller motsvarande.";

    const EN: &str = "Syllabus
for course at first level
Course code:                                   IB130N
Main field:                                    Computer and Systems Sciences
Specialisation:                                G1N - First cycle, has only upper-secondary level entry requirements

Course structure
Examination code     Name                                                                          Higher Education Credits
IDSI                 Introduction to Computer and Systems Sciences, assignment                                          3.5
IDST                 Introduction to Computer and Systems Sciences, exam                                                  4
Course content";

    #[test]
    fn reads_level_and_modules_from_a_swedish_plan() {
        let facts = parse(&[SV.to_string()]);
        assert_eq!(facts.level.as_deref(), Some("G2F"));
        assert_eq!(
            facts.modules,
            vec![
                PlanModule {
                    code: "PR2T".into(),
                    name: "Programmering 2, tenta".into(),
                    credits: 4.0,
                },
                PlanModule {
                    code: "PR2I".into(),
                    name: "Programmering 2, inlämningsuppgift".into(),
                    credits: 3.5,
                },
            ]
        );
    }

    #[test]
    fn reads_level_and_modules_from_an_english_plan() {
        // The English table runs straight into the next heading.
        let facts = parse(&[EN.to_string()]);
        assert_eq!(facts.level.as_deref(), Some("G1N"));
        assert_eq!(facts.modules.len(), 2);
        assert_eq!(facts.modules[0].code, "IDSI");
        assert_eq!(facts.modules[1].credits, 4.0);
    }

    #[test]
    fn a_table_split_across_pages_is_still_read() {
        let (head, tail) = SV.split_at(SV.find("PR2I").unwrap());
        let head = format!("{head}\n                                          Sidan 1/2\n");
        let facts = parse(&[head, tail.to_string()]);
        assert_eq!(facts.modules.len(), 2);
    }

    #[test]
    fn missing_fields_stay_empty() {
        let facts = parse(&["Something that is not a plan".to_string()]);
        assert_eq!(facts, PlanFacts::default());
    }

    #[test]
    fn decimal_commas_are_read() {
        let text = "Provkod   Benämning                 Högskolepoäng\nAB12      Del ett                   7,5\n";
        assert_eq!(parse(&[text.to_string()]).modules[0].credits, 7.5);
    }
}
