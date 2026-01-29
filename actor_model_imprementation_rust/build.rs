// アセンブリファイルのコンパイルとリンク用ファイル

use std::process::Command;

const ASM_FILE: &str = "asm/context.s";
const O_FILE: &str = "asm/context.o";
const LIB_FILE: &str = "asm/libcontext.a";

/*  このファイルは以下のコマンドと等価
    $ cc asm/context.s -c -fPIC -o asm/context.o
    $ ar crus asm/libcontext.a asm/context.o

    ar: 静的ライブラリの作成・ファイル取り出しなどを行うコマンド
    option: 
        c: asm/libcontext.a (書庫) を新たに作成
        r: 書庫にファイルを挿入; 同名のファイルがあれば置き換え
        u: 挿入するファイルより書庫のファイルが古い場合のみ置き換え
        s: 索引を書庫に書き込み

    => 作成された asm/libcontext.a をリンクしてコンパイル
*/

fn main() {
    Command::new("cc").args(&[ASM_FILE, "-c", "-fPIC", "-o"]).arg(O_FILE).status().unwrap();
    Command::new("ar").args(&["crus", LIB_FILE, O_FILE]).status().unwrap();
    println!("cargo:rustc-link-search=native={}", "asm");       // asm をライブラリ検索 pass に追加
    println!("cargo:rustc-link-lib=static=context");            // libcontext.a という静的ライブラリをリンク
    println!("cargo:rerun-if-changed=asm/context.s");           // asm/context.s というファイルに依存
}