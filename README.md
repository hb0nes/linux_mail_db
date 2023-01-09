# Readme
Linux MailDB will create an in-memory DB (using FxHashMap) by reading the configured mail files once upon starting.  
It will then start tailing the configured tail files for further updates.  

The DB can be queried, e.g.:
```
curl 'localhost:8080/find_mail?email_address_filter=test@email.com'
curl 'localhost:8080/find_mail?email_address_filter=test@email.com?subject_filter=test subject'
```

Or, to retrieve all mails that have a subject:
```
curl 'localhost:8080/find_mail?email_address_filter=test@email.com?subject_filter='
```

## Building
Building happens with buildx due to heredoc contained in Dockerfile.
```
docker buildx build -t linux-mail-db .
```

## Disclaimer
I've made this to assist sys admins or support teams in debugging mail issues.  
It is still early days for me when it comes to Rust, so there will be plenty of things that are suboptimal.  
Nonetheless, it works, and it's fast.  
