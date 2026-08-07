use dmap::DHashMap;

use crate::mc::Register;

pub struct ParcopySolver<R: Register> {
    ready: Vec<R>,
    todo: Vec<R>,
    pred: DHashMap<u8, R>,
    loc: DHashMap<u8, Option<R>>,
}

impl<R: Register> Default for ParcopySolver<R> {
    fn default() -> Self {
        Self {
            ready: Vec::new(),
            todo: Vec::new(),
            pred: DHashMap::default(),
            loc: DHashMap::default(),
        }
    }
}

impl<R: Register> ParcopySolver<R> {
    pub fn new() -> Self {
        Self::default()
    }
}

impl<R: Register> ParcopySolver<R> {
    /// Takes a list of argument registers as alternating [to, from] pairs
    /// and sequentializes them, emitting copies by calling
    /// emit_copy(to, from) repeatedly.
    pub fn parcopy(
        &mut self,
        args: impl Clone + IntoIterator<Item = R>,
        mut emit_copy: impl FnMut(R, R),
        tmp: R,
    ) {
        let Self {
            ready,
            todo,
            pred,
            loc,
        } = self;

        ready.clear();
        todo.clear();
        pred.clear();
        loc.clear();

        // SOURCE: https://github.com/pfalcon/parcopy/blob/f38848a1da69012904a16c9bc529ce58dba3cd44/parcopy1.py

        for b in args.clone().into_iter().step_by(2) {
            loc.insert(b.bit_index(), None);
        }

        {
            let mut args = args.clone().into_iter();

            while let Some((b, a)) = args.next().map(|b| (b, args.next().unwrap())) {
                loc.insert(a.bit_index(), Some(a));
                pred.insert(b.bit_index(), a);

                debug_assert!(
                    !todo.iter().any(|x| x.bit_index() == b.bit_index()),
                    "Conflicting assignment destinations in parcopy"
                );

                todo.push(b);
            }
        }

        for b in args.into_iter().step_by(2) {
            if loc[&b.bit_index()].is_none() {
                ready.push(b);
            }
        }

        while !todo.is_empty() {
            while let Some(b) = ready.pop() {
                if !pred.contains_key(&b.bit_index()) {
                    continue;
                }

                let a = pred[&b.bit_index()];
                let c = loc[&a.bit_index()].unwrap();

                emit_copy(b, c);

                if let Some(i) = todo.iter().position(|x| x.bit_index() == c.bit_index()) {
                    // PERF: remove from Vec
                    todo.remove(i);
                }

                loc.insert(a.bit_index(), Some(b));

                if a.bit_index() == c.bit_index() {
                    ready.push(a);
                }
            }

            let Some(b) = todo.pop() else {
                break;
            };

            let a = pred[&b.bit_index()];
            let c = loc[&a.bit_index()].unwrap();

            if b.bit_index() != c.bit_index() {
                let d = loc[&b.bit_index()].unwrap();
                emit_copy(tmp, d);
                loc.insert(b.bit_index(), Some(tmp));
                ready.push(b);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ParcopySolver;
    use crate::mc::UnknownRegister;

    const fn r(i: u32) -> UnknownRegister {
        UnknownRegister(i)
    }

    const TMP: u32 = 99;

    fn case<const N: usize, const M: usize>(inp: [(u32, u32); N], exp: [(u32, u32); M]) {
        let mut res = Vec::new();
        ParcopySolver::new().parcopy(
            inp.into_iter().flat_map(|(a, b)| [r(a), r(b)]),
            |a, b| res.push((a, b)),
            r(TMP),
        );
        let exp: Vec<(UnknownRegister, UnknownRegister)> =
            exp.into_iter().map(|(a, b)| (r(a), r(b))).collect();
        assert_eq!(exp, res);
    }

    #[test]
    fn parcopy_works() {
        // 0 <- 1, 1 <- 2 can stay in this order
        case([(0, 1), (1, 2)], [(0, 1), (1, 2)]);
        // order gets swapped here
        case([(1, 2), (0, 1)], [(0, 1), (1, 2)]);

        case([(0, 1), (1, 0)], [(TMP, 1), (1, 0), (0, TMP)]);
        case([(1, 0), (0, 1)], [(TMP, 0), (0, 1), (1, TMP)]);
        case(
            [(0, 1), (1, 2), (2, 0)],
            [(TMP, 2), (2, 0), (0, 1), (1, TMP)],
        );
        case(
            [(0, 1), (1, 0), (2, 3), (3, 2)],
            [(TMP, 3), (3, 2), (2, TMP), (TMP, 1), (1, 0), (0, TMP)],
        );
    }

    #[test]
    #[should_panic]
    fn parcopy_invalid() {
        // duplicated assignment should fail (in debug mode)
        case([(0, 1), (0, 2)], []);
    }
}
