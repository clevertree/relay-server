import vm from 'node:vm';
import fs from 'node:fs';
import path from 'node:path';
async function main() {
    const context = JSON.parse(fs.readFileSync(0).toString());
    const branchHash = Buffer.from(context.branch || 'main').toString('hex').slice(0, 12);
    const dbPath = path.join(context.repo_path, '.relay_data', 'branches', branchHash, 'index.db.json');
    if (!fs.existsSync(path.dirname(dbPath))) fs.mkdirSync(path.dirname(dbPath), { recursive: true });
    fs.writeFileSync(dbPath, JSON.stringify({
        metadata: { indexed_head: context.new_commit },
        collections: { index: [ { title: "Test Item", _id: 1 } ] }
    }));
    process.exit(0);
}
main();
