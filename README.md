# Rust libp2p and tokio demo
This repository contains code to show basic usage of the async programming using [tokio](https://tokio.rs/) and 
p2p - [libp2p](https://libp2p.io/) library in 
Rust. The docs for the crate can be found [here](https://crates.io/crates/libp2p).
This code base contains a simple implementation demonstrating the use.
Here we have a simple stdio based implementation that lets peers create and share *Memos*.
A memo is a simple note with a title and body. Notes can be public if you publish the *Memo* or private.
Public *Memos* can be looked up by any peer and can also be shared with peers.

## Showcasing Fundamentals and Concepts
- Rust fundamentals
- async programming
- Using libp2p
- Using the noise protocol for encryption.

## Docs 
Docs can be found in the docs folder and on [github pages](https://anantasty.github.io/rust-libp2p-demo/p2p/).

## Running the code
Currently there is nopublished binaries. We need to run this on two or more nodes on the same network.
We can run it using :
```
RUST_LOG=info cargo run
```
The following commands are available via stdin.
- `ls p` - List Peers.
- `ls m` - List local memos.
- `ls m all` - List all public memos from known peers.
- `ls m {peer_id}` - List all public memos from peer specified by peer_id.
- `create m Title|Body` - Create a new memo.
- `publish m {id}` - Set the public flag on the memo and publish to all known

## Limitations
This is a very simple and not considered production worthy or secure.
- No implementation of commandline parsing to select the storage json for Memo.json
- The implementation of autoincrementing id's for memos is purely local and does not take into account id collisions between peers etc.
- Security - Although the communication between peers is encrypted very little is done in the way of authenticating peers.

