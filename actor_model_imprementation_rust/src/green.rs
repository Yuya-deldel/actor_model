use nix::sys::mman::{mprotect, ProtFlags};
use rand;
use std::alloc::{alloc, dealloc, Layout};
use std::collections::{HashMap, HashSet, LinkedList};
use std::ffi::c_void;
use std::ptr;

/*  AArch64 のレジスタ
    x0 ~ x30: 汎用 64bit register
    v0 ~ v31: SIMD register (下位 64bit は d0 ~ d30 register)
    sp: スタックポインタ

    呼び出し規則 (AAPCS64)
    x30: リンクレジスタ                                   => 関数リターン時の戻りアドレスを保存する (ret 命令で使われる)
    x29: フレームレジスタ                                 => スタック領域の好きな場所を指せる
    x19 ~ x28: callee (呼び出された関数) 側保存レジスタ    => 関数からリターンする前に復帰しなければならない
    x18: プラットフォームレジスタ                          => OS 依存 (ユーザーが使うことは非推奨)
    x17: 第二プロシージャ呼び出し間一時レジスタ             => リンカによって使われる
    x16: 第一プロシージャ呼び出し間一時レジスタ             => リンカによって使われる
    x9 ~ x15: 一時レジスタ
    x8: 返り値用レジスタ (アドレス渡し用)                  => 大きなデータを渡したい時用
    x0 ~ x7: 引数・返り値用レジスタ

    v16 ~ v31: 一時レジスタ
    d8 ~ d15: callee 保存レジスタ                         => 関数からリターンする前に復帰しなければならない
    d0 ~ d7: 引数・返り値用レジスタ
*/

// Registers
#[repr(C)]  // 内部メモリ表現が C 言語のそれと同じになるように設定 -> アセンブリで定義した関数に渡す
struct Registers {      
    // callee 保存レジスタ: callee 側が責任をもって保存しなければならない -> heap 上に退避
    d8: u64, d9: u64, d10: u64, d11: u64, d12: u64, d13: u64, d14: u64, d15: u64,
    x19: u64, x20: u64, x21: u64, x22: u64, x23: u64, x24: u64, x25: u64, x26: u64, x27: u64, x28: u64,
    x30: u64,   // リンクレジスタ: コンテキストスイッチから戻ってくるために必要
    sp: u64,    // スタックポインタ: スタック復元のために必要
}   // それ以外のレジスタはスタック上に退避する

impl Registers {
    fn new(sp: u64) -> Self {
        Registers { 
            d8: 0, d9: 0, d10: 0, d11: 0, d12: 0, d13: 0, d14: 0, d15: 0, 
            x19: 0, x20: 0, x21: 0, x22: 0, x23: 0, x24: 0, x25: 0, x26: 0, x27: 0, x28: 0, 
            x30: entry_point as u64,    // コンテキストスイッチされた際に entry_point 関数が最初に呼び出されるようにする 
            sp, 
        }
    }
}

extern "C" {
    fn set_context(ctx: *mut Registers) -> u64;         // 返り値が 0 => set_context からの返り値; 返り値が 1 => switch_context が呼ばれた
    fn switch_context(ctx: *const Registers) -> !;      // !: never 型: return しないことを示す
}

// Context
type Entry = fn();      // スレッド開始時に実行する関数の型
const PAGE_SIZE: usize = 4 * 1024;      // 4KiB: Linux の仮想メモリ
struct Context {
    regs: Registers,
    stack: *mut u8,
    stack_layout: Layout,   // dealloc() するために必要
    entry: Entry,
    thread_id: u64,
} 

impl Context {
    fn get_regs_mut(&mut self) -> *mut Registers {      // Registers へのポインタ
        &mut self.regs as *mut Registers
    }

    fn get_regs(&self) -> *const Registers {
        &self.regs as *const Registers
    }

    fn new(func: Entry, stack_size: usize, thread_id: u64) -> Self {
        let layout = Layout::from_size_align(stack_size, PAGE_SIZE).unwrap();   // PAGE_SIZE にアライメントされたメモリレイアウトを指定
        let stack = unsafe {alloc(layout)};     // スタック用メモリ領域を確保
        unsafe {mprotect(stack as *mut c_void, PAGE_SIZE, ProtFlags::PROT_NONE).unwrap()};  // スタックオーバーフロー検出用のガードページを設定

        let regs = Registers::new(stack as u64 + stack_size as u64);    // Registers 構造体の初期化

        Context { 
            regs: regs, 
            stack: stack, 
            stack_layout: layout,  
            entry: func, 
            thread_id: thread_id, 
        }
    }
}

// map: key_of_actor -> LinkedList<Message>: actor ごとの message queue
struct MappedList<T> {
    map: HashMap<u64, LinkedList<T>>,
}

impl<T> MappedList<T> {
    fn new() -> Self {
        MappedList { map: HashMap::new() }
    }

    fn push_back(&mut self, key: u64, val: T) {
        if let Some(list) = self.map.get_mut(&key) {    // 対応する actor が存在すれば val を追加 (push back)
            list.push_back(val);
        } else {        // actor が存在しなければ、新たに追加
            let mut list = LinkedList::new();
            list.push_back(val);
            self.map.insert(key, list);
        }
    }

    fn pop_front(&mut self, key: u64) -> Option<T> {        // key に対応するリストから取り出す (pop front)
        if let Some(list) = self.map.get_mut(&key) {
            let val = list.pop_front();
            if list.len() == 0 {
                self.map.remove(&key);
            }

            val
        } else {
            None 
        }
    }

    fn clear(&mut self) {
        self.map.clear();
    }
}

// マルチスレッド化する場合には mutex などで保護する必要がある; 簡単のため global 変数を用いる
static mut CTX_MAIN: Option<Box<Registers>> = None;     // main() のコンテキスト
static mut UNUSED_STACK: (*mut u8, Layout) = (ptr::null_mut(), Layout::new::<u8>());    // free() すべきスタック領域へのポインタとレイアウト
static mut CONTEXTS: LinkedList<Box<Context>> = LinkedList::new();      // threads queue
static mut ID: *mut HashSet<u64> = ptr::null_mut();     // thread id の集合
static mut MESSAGES: *mut MappedList<u64> = ptr::null_mut();
static mut WAITING: *mut HashMap<u64, Box<Context>> = ptr::null_mut();

fn get_id() -> u64 {
    loop {
        let rnd = rand::random::<u64>();
        unsafe {
            if !(*ID).contains(&rnd) {
                (*ID).insert(rnd);
                return rnd;
            }
        }
    }
}

pub fn spawn(func: Entry, stack_size: usize) -> u64 {
    unsafe {
        let id = get_id();
        CONTEXTS.push_back(Box::new(Context::new(func, stack_size, id)));   // queue の最後尾に新規作成
        schedule();     // コンテキストスイッチ
        id
    }
}

// main() から一度だけ呼ばれ、グローバル変数の初期化と解放を行う
pub fn spawn_from_main(func: Entry, stack_size: usize) {
    unsafe {
        if let Some(_) = &CTX_MAIN {
            panic!("spawn_from_main is called twice");
        }

        // main() 関数用のコンテキストを生成
        CTX_MAIN = Some(Box::new(Registers::new(0)));
        if let Some(ctx) = &mut CTX_MAIN {
            // global 変数の初期化
            let mut msgs = MappedList::new();
            MESSAGES = &mut msgs as *mut MappedList<u64>;
            let mut waiting = HashMap::new();
            WAITING = &mut waiting as *mut HashMap<u64, Box<Context>>;
            let mut ids = HashSet::new();
            ID = &mut ids as *mut HashSet<u64>;
        
            // CONTEXTS の初期化 + func の thread を起動
            if set_context(&mut **ctx as *mut Registers) == 0 {     // main() のコンテキスト保存
                CONTEXTS.push_back(Box::new(Context::new(func, stack_size, get_id())));
                let first = CONTEXTS.front().unwrap();
                switch_context(first.get_regs());       // func 実行
            }   // func() からリターンして main() に戻ってきた

            // 後処理
            rm_unused_stack();      // 不要なスタック解放
            CTX_MAIN = None;
            CONTEXTS.clear();
            MESSAGES = ptr::null_mut();
            WAITING = ptr::null_mut();
            ID = ptr::null_mut();

            // msgs, waiting, ids を明示的にリセット -> ライフタイムを保証
            msgs.clear();
            waiting.clear();
            ids.clear();
        }
    }
}

pub fn schedule() {
    unsafe {
        if CONTEXTS.len() == 1 {
            return;
        }

        // queue からコンテキストを pop_front -> push_back
        let mut ctx = CONTEXTS.pop_front().unwrap();
        let regs = ctx.get_regs_mut();      // get register data
        CONTEXTS.push_back(ctx);

        if set_context(regs) == 0 {     // 今の実行プロセスの状態を保存; 
            let next = CONTEXTS.front().unwrap();
            switch_context((**next).get_regs());    // コンテキストスイッチ
        }

        rm_unused_stack();      // 不要なスタック領域を削除
    }
}

unsafe fn rm_unused_stack() {
    if UNUSED_STACK.0 != ptr::null_mut() {
        mprotect(UNUSED_STACK.0 as *mut c_void, PAGE_SIZE, ProtFlags::PROT_READ | ProtFlags::PROT_WRITE).unwrap();
        dealloc(UNUSED_STACK.0, UNUSED_STACK.1);
        UNUSED_STACK = (ptr::null_mut(), Layout::new::<u8>());
    }
}

// actor 間の message のやり取り
pub fn send(key: u64, msg: u64) {
    unsafe {    
        // message 送信
        (*MESSAGES).push_back(key, msg);
        if let Some(ctx) = (*WAITING).remove(&key) {
            CONTEXTS.push_back(ctx);
        }
    }
    schedule();     // 協調的マルチタスク: actor 側が scheduling 実行
}

pub fn receive() -> Option<u64> {
    unsafe {
        let key = CONTEXTS.front().unwrap().thread_id;      // thread_id
        
        if let Some(msg) = (*MESSAGES).pop_front(key) {     // message がすでに queue に存在する
            return Some(msg);
        }   // 以下、message が queue に存在しない

        if CONTEXTS.len() == 1 {    // 実行可能スレッドがほかに存在しない -> deadlock    
            panic!("deadlock");     // 実際の設計ではタイムアウトを設けて処理
        }

        // このスレッドを受信待ち状態にし、コンテキストスイッチ
        let mut ctx = CONTEXTS.pop_front().unwrap();
        let regs = ctx.get_regs_mut();
        (*WAITING).insert(key, ctx);
        if set_context(regs) == 0 {
            let next = CONTEXTS.front().unwrap();
            switch_context((**next).get_regs());
        }   // return しない

        // 以下は疑似覚醒対策
        rm_unused_stack();
        (*MESSAGES).pop_front(key)
    }
}

// entry_point 関数
extern "C" fn entry_point() {
    unsafe {
        let ctx = CONTEXTS.front().unwrap();
        ((**ctx).entry)();      // thread の entry 関数実行 
        // entry() の終了 <=> thread の終了
        
        // thread 終了時の処理
        let ctx = CONTEXTS.pop_front().unwrap();
        (*ID).remove(&ctx.id);
        UNUSED_STACK = ((*ctx).stack, (*ctx).stack_layout);     // コンテキストスイッチ後にスタック領域を解放するよう予約

        match CONTEXTS.front() {        // 次のスレッドにコンテキストスイッチ
            Some(c) => {
                switch_context((**c).get_regs());
            },
            None => {       // main() へコンテキストスイッチ
                if let Some(c) = &CTX_MAIN {
                    switch_context(&**c as *const Registers);
                }
            }
        };
    }
    panic!("entry point");      // 到達しないはず
}