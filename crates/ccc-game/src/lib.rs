//! Deterministic tic-tac-toe. No AI.

pub mod evidence;
pub mod evidence_jsonl;

use ccc_core::{Error, Mark};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cell {
    Empty,
    Occupied(Mark),
}

impl Cell {
    pub fn glyph(self, square: u8) -> char {
        match self {
            Cell::Empty => char::from(b'0' + square),
            Cell::Occupied(m) => m.glyph(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    InProgress,
    Win(Mark),
    Draw,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Board {
    cells: [Cell; 9],
    ply: u8,
}

impl Default for Board {
    fn default() -> Self {
        Self::new()
    }
}

impl Board {
    pub fn new() -> Self {
        Self {
            cells: [Cell::Empty; 9],
            ply: 0,
        }
    }

    pub fn ply(&self) -> u8 {
        self.ply
    }

    pub fn turn(&self) -> Mark {
        if self.ply.is_multiple_of(2) {
            Mark::X
        } else {
            Mark::O
        }
    }

    pub fn cell(&self, square: u8) -> Result<Cell, Error> {
        Ok(self.cells[index(square)?])
    }

    pub fn outcome(&self) -> Outcome {
        const LINES: [[usize; 3]; 8] = [
            [0, 1, 2],
            [3, 4, 5],
            [6, 7, 8],
            [0, 3, 6],
            [1, 4, 7],
            [2, 5, 8],
            [0, 4, 8],
            [2, 4, 6],
        ];
        for line in LINES {
            if let (Cell::Occupied(a), Cell::Occupied(b), Cell::Occupied(c)) = (
                self.cells[line[0]],
                self.cells[line[1]],
                self.cells[line[2]],
            ) {
                if a == b && b == c {
                    return Outcome::Win(a);
                }
            }
        }
        if self.cells.iter().all(|c| !matches!(c, Cell::Empty)) {
            Outcome::Draw
        } else {
            Outcome::InProgress
        }
    }

    pub fn play(&mut self, square: u8, mark: Mark) -> Result<Outcome, Error> {
        if self.outcome() != Outcome::InProgress {
            return Err(Error::IllegalMove("game is already over".into()));
        }
        if mark != self.turn() {
            return Err(Error::IllegalMove(format!(
                "it is {:?}'s turn",
                self.turn()
            )));
        }
        let i = index(square)?;
        if !matches!(self.cells[i], Cell::Empty) {
            return Err(Error::IllegalMove(format!("square {square} occupied")));
        }
        self.cells[i] = Cell::Occupied(mark);
        self.ply += 1;
        Ok(self.outcome())
    }

    pub fn legal_squares(&self) -> Vec<u8> {
        if self.outcome() != Outcome::InProgress {
            return Vec::new();
        }
        (1..=9)
            .filter(|&s| matches!(self.cells[(s - 1) as usize], Cell::Empty))
            .collect()
    }

    pub fn render(&self) -> String {
        let g = |s: u8| self.cells[(s - 1) as usize].glyph(s);
        format!(
            " {} | {} | {}\n---+---+---\n {} | {} | {}\n---+---+---\n {} | {} | {}",
            g(1),
            g(2),
            g(3),
            g(4),
            g(5),
            g(6),
            g(7),
            g(8),
            g(9)
        )
    }

    pub fn compact(&self) -> String {
        self.cells
            .iter()
            .enumerate()
            .map(|(i, c)| match c {
                Cell::Empty => char::from(b'1' + i as u8),
                Cell::Occupied(m) => m.glyph(),
            })
            .collect()
    }
}

fn index(square: u8) -> Result<usize, Error> {
    if (1..=9).contains(&square) {
        Ok((square - 1) as usize)
    } else {
        Err(Error::IllegalMove(format!("square {square} is not 1-9")))
    }
}
