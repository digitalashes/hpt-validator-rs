// Runs upstream's CSV unit tests with lift.setup.mjs, which records every direct call to
// CsvValidator's row-level methods so cases.mjs can turn each into a whole CSV file.
export default {
  test: {
    globals: true,
    include: ["test/validators/CsvValidator*.test.ts"],
    setupFiles: [new URL("./lift.setup.mjs", import.meta.url).pathname],
  },
};
